use std::{collections::HashMap, fs, path::Path};

use framehop::{CacheNative, MayAllocateDuringUnwind, Module, Unwinder, UnwinderNative};
use framehop_object::ObjectSectionInfo;
use mperf_data::{CallFrame, Event, ProcMapEntry, UserRegs};

type NativeUnwinder = UnwinderNative<Vec<u8>, MayAllocateDuringUnwind>;
type NativeCache = CacheNative<MayAllocateDuringUnwind>;

/// Per-process module tables and caches used only after recording has completed.
pub(crate) struct PostHocUnwinder {
    unwinders: HashMap<u32, NativeUnwinder>,
    caches: HashMap<u32, NativeCache>,
    last_stack: Option<(u128, smallvec::SmallVec<[CallFrame; 32]>)>,
}

impl PostHocUnwinder {
    pub(crate) fn new(proc_maps: &[ProcMapEntry]) -> Self {
        // Coalesce the individual executable mappings for one loaded object. framehop
        // needs the object load bias plus a runtime range; both come from ProcMapEntry.
        let mut ranges = HashMap::<(u32, String, u64), (u64, u64)>::new();
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

        let mut unwinders = HashMap::<u32, NativeUnwinder>::new();
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
            caches: HashMap::new(),
            last_stack: None,
        }
    }

    /// Apply the milestone fallback chain: DWARF, sampled callchain, then raw IP.
    pub(crate) fn unwind_event(&mut self, event: &mut Event) {
        if event.user_regs.is_some() {
            if let Some(stack) = self.unwind(event) {
                event.callstack = stack.into_iter().map(CallFrame::IP).collect();
            } else if event.callstack.is_empty() {
                if let Some(ip) = event.user_regs.as_ref().and_then(instruction_pointer) {
                    event.callstack.push(CallFrame::IP(ip));
                }
            }
            self.last_stack = Some((event.correlation_id, event.callstack.clone()));
        } else if let Some((correlation_id, stack)) = &self.last_stack {
            if *correlation_id == event.correlation_id {
                event.callstack.clone_from(stack);
            }
        }
    }

    fn unwind(&mut self, event: &Event) -> Option<Vec<u64>> {
        let regs = event.user_regs.as_ref()?;
        if event.user_stack.is_empty() {
            return None;
        }
        let initial_regs = native_regs(regs)?;
        let stack_pointer = native_stack_pointer(regs)?;
        let pc = instruction_pointer(regs)?;
        let unwinder = self.unwinders.get(&event.process_id)?;
        let cache = self.caches.entry(event.process_id).or_default();
        let stack = &event.user_stack;
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

fn register(regs: &UserRegs, index: u32) -> Option<u64> {
    let bit = 1_u64.checked_shl(index)?;
    if regs.mask & bit == 0 {
        return None;
    }
    let value_index = (regs.mask & bit.wrapping_sub(1)).count_ones() as usize;
    regs.values.get(value_index).copied()
}

#[cfg(target_arch = "x86_64")]
fn instruction_pointer(regs: &UserRegs) -> Option<u64> {
    register(regs, 8)
}

#[cfg(target_arch = "x86_64")]
fn native_stack_pointer(regs: &UserRegs) -> Option<u64> {
    register(regs, 7)
}

#[cfg(target_arch = "x86_64")]
fn native_regs(regs: &UserRegs) -> Option<framehop::UnwindRegsNative> {
    Some(framehop::x86_64::UnwindRegsX86_64::new(
        instruction_pointer(regs)?,
        native_stack_pointer(regs)?,
        register(regs, 6)?,
    ))
}

#[cfg(target_arch = "aarch64")]
fn instruction_pointer(regs: &UserRegs) -> Option<u64> {
    register(regs, 32)
}

#[cfg(target_arch = "aarch64")]
fn native_stack_pointer(regs: &UserRegs) -> Option<u64> {
    register(regs, 31)
}

#[cfg(target_arch = "aarch64")]
fn native_regs(regs: &UserRegs) -> Option<framehop::UnwindRegsNative> {
    Some(framehop::aarch64::UnwindRegsAarch64::new(
        register(regs, 30)?,
        native_stack_pointer(regs)?,
        register(regs, 29)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::register;
    use mperf_data::UserRegs;

    #[test]
    fn finds_perf_register_by_mask_order() {
        let regs = UserRegs {
            abi: 2,
            mask: (1 << 2) | (1 << 7) | (1 << 8),
            values: vec![20, 70, 80],
        };
        assert_eq!(register(&regs, 2), Some(20));
        assert_eq!(register(&regs, 7), Some(70));
        assert_eq!(register(&regs, 8), Some(80));
        assert_eq!(register(&regs, 6), None);
    }
}
