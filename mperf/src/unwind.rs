use std::{collections::BTreeMap, fs, path::Path};

use framehop::{CacheNative, MayAllocateDuringUnwind, Module, Unwinder, UnwinderNative};
use framehop_object::ObjectSectionInfo;
use mperf_data::ProcMapEntry;
use smallvec::SmallVec;

use crate::postprocess::RawSample;

type NativeUnwinder = UnwinderNative<Vec<u8>, MayAllocateDuringUnwind>;
type NativeCache = CacheNative<MayAllocateDuringUnwind>;
type Frames = SmallVec<[u64; 32]>;

/// Per-process module tables and caches used only after recording has completed.
pub(crate) struct PostHocUnwinder {
    unwinders: BTreeMap<u32, NativeUnwinder>,
    caches: BTreeMap<u32, NativeCache>,
    last_stack: Option<(u64, Frames)>,
}

impl PostHocUnwinder {
    pub(crate) fn new(proc_maps: &[ProcMapEntry]) -> Self {
        // Coalesce the individual executable mappings for one loaded object. framehop
        // needs the object load bias plus a runtime range; both come from ProcMapEntry.
        // A sorted map makes module insertion reproducible. Correct recordings
        // should have only one load bias per mapping, but old result sets may
        // contain conflicting overlapping entries and must not change meaning
        // randomly from one `mperf show` invocation to the next.
        let mut ranges = BTreeMap::<(u32, String, u64), (u64, u64)>::new();
        for map in proc_maps {
            if map.filename.is_empty() || map.filename.starts_with('[') || map.size == 0 {
                continue;
            }
            let start = map.address as u64;
            let end = start.saturating_add(map.size as u64);
            let base = start.saturating_sub(map.offset as u64);
            ranges
                .entry((map.pid, map.filename.clone(), base))
                .and_modify(|range| {
                    range.0 = range.0.min(start);
                    range.1 = range.1.max(end);
                })
                .or_insert((start, end));
        }

        let mut unwinders = BTreeMap::<u32, NativeUnwinder>::new();
        for ((pid, filename, base), (start, end)) in ranges {
            let Ok(bytes) = fs::read(Path::new(&filename)) else {
                continue;
            };
            let Ok(object) = object::File::parse(bytes.as_slice()) else {
                continue;
            };
            let module = Module::<Vec<u8>>::new(
                filename,
                start..end,
                base,
                ObjectSectionInfo::from_ref(&object),
            );
            unwinders.entry(pid).or_default().add_module(module);
        }

        Self {
            unwinders,
            caches: BTreeMap::new(),
            last_stack: None,
        }
    }

    /// Apply the milestone fallback chain: DWARF, sampled callchain, then raw IP.
    pub(crate) fn resolve(&mut self, sample: &RawSample<'_>) -> Frames {
        let mut frames = Frames::from_slice(sample.callchain);
        if sample.regs_mask != 0 {
            if let Some(stack) = self.unwind(sample) {
                frames = stack.into_iter().collect();
            } else if frames.is_empty()
                && let Some(ip) = instruction_pointer(sample)
            {
                frames.push(ip);
            }
            self.last_stack = Some((sample.group_id, frames.clone()));
        } else if let Some((group_id, stack)) = &self.last_stack
            && *group_id == sample.group_id
        {
            frames.clone_from(stack);
        }
        frames
    }

    fn unwind(&mut self, sample: &RawSample<'_>) -> Option<Vec<u64>> {
        if sample.user_stack.is_empty() {
            return None;
        }
        let initial_regs = native_regs(sample)?;
        let stack_pointer = native_stack_pointer(sample)?;
        let pc = instruction_pointer(sample)?;
        let unwinder = self.unwinders.get(&sample.pid)?;
        let cache = self.caches.entry(sample.pid).or_default();
        let stack = sample.user_stack;
        let mut read_stack = |address: u64| -> Result<u64, ()> {
            let offset = address.checked_sub(stack_pointer).ok_or(())? as usize;
            let bytes: [u8; 8] = stack
                .get(offset..offset.checked_add(8).ok_or(())?)
                .ok_or(())?
                .try_into()
                .map_err(|_| ())?;
            Ok(u64::from_ne_bytes(bytes))
        };
        let mut iter = unwinder.iter_frames(pc, initial_regs, cache, &mut read_stack);
        let mut frames = Vec::new();
        while frames.len() < 512 {
            match iter.next() {
                Ok(Some(frame)) => frames.push(frame.address()),
                Ok(None) | Err(_) => break,
            }
        }
        // A lone PC did not actually unwind; retain the kernel callchain instead.
        (frames.len() > 1).then_some(frames)
    }
}

fn register(sample: &RawSample<'_>, index: u32) -> Option<u64> {
    let bit = 1_u64.checked_shl(index)?;
    if sample.regs_mask & bit == 0 {
        return None;
    }
    let value_index = (sample.regs_mask & bit.wrapping_sub(1)).count_ones() as usize;
    sample.regs.get(value_index).copied()
}

#[cfg(target_arch = "x86_64")]
fn instruction_pointer(sample: &RawSample<'_>) -> Option<u64> {
    register(sample, 8)
}

#[cfg(target_arch = "x86_64")]
fn native_stack_pointer(sample: &RawSample<'_>) -> Option<u64> {
    register(sample, 7)
}

#[cfg(target_arch = "x86_64")]
fn native_regs(sample: &RawSample<'_>) -> Option<framehop::UnwindRegsNative> {
    Some(framehop::x86_64::UnwindRegsX86_64::new(
        instruction_pointer(sample)?,
        native_stack_pointer(sample)?,
        register(sample, 6)?,
    ))
}

#[cfg(target_arch = "aarch64")]
fn instruction_pointer(sample: &RawSample<'_>) -> Option<u64> {
    register(sample, 32)
}

#[cfg(target_arch = "aarch64")]
fn native_stack_pointer(sample: &RawSample<'_>) -> Option<u64> {
    register(sample, 31)
}

#[cfg(target_arch = "aarch64")]
fn native_regs(sample: &RawSample<'_>) -> Option<framehop::UnwindRegsNative> {
    Some(framehop::aarch64::UnwindRegsAarch64::new(
        register(sample, 30)?,
        native_stack_pointer(sample)?,
        register(sample, 29)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{RawSample, register};

    #[test]
    fn finds_perf_register_by_mask_order() {
        let sample = RawSample {
            timestamp: 0,
            pid: 1,
            tid: 1,
            cpu: 0,
            group_id: 0,
            event_id: 0,
            value: 0,
            time_enabled: 0,
            time_running: 0,
            ip: 0,
            callchain: &[],
            lbr_callchain: &[],
            regs_mask: (1 << 2) | (1 << 7) | (1 << 8),
            regs: &[20, 70, 80],
            user_stack: &[],
        };
        assert_eq!(register(&sample, 2), Some(20));
        assert_eq!(register(&sample, 7), Some(70));
        assert_eq!(register(&sample, 8), Some(80));
        assert_eq!(register(&sample, 6), None);
    }
}
