use std::collections::HashMap;

use mperf_data::{EventType, ProcMapEntry};
use object::{Object, ObjectSegment};
use pmu::Counter;

pub fn counter_to_event_ty(counter: &Counter) -> EventType {
    match counter {
        Counter::Cycles => EventType::PmuCycles,
        Counter::Instructions => EventType::PmuInstructions,
        Counter::LLCReferences => EventType::PmuLlcReferences,
        Counter::LLCMisses => EventType::PmuLlcMisses,
        Counter::BranchInstructions => EventType::PmuBranchInstructions,
        Counter::BranchMisses => EventType::PmuBranchMisses,
        Counter::StalledCyclesFrontend => EventType::PmuStalledCyclesFrontend,
        Counter::StalledCyclesBackend => EventType::PmuStalledCyclesBackend,
        Counter::CpuClock => EventType::OsCpuClock,
        Counter::PageFaults => EventType::OsPageFaults,
        Counter::CpuMigrations => EventType::OsCpuMigrations,
        Counter::ContextSwitches => EventType::OsContextSwitches,
        Counter::Custom(_) => EventType::PmuCustom,
        Counter::Internal {
            name: _,
            desc: _,
            code: _,
        } => EventType::PmuCustom,
    }
}

#[allow(dead_code)]
pub struct ResolvedPME<'a> {
    loader: Option<addr2line::Loader>,
    name: &'a str,
    address: usize,
    size: usize,
    offset: usize,
    object_ranges: Vec<(u64, u64)>,
}

impl ResolvedPME<'_> {
    fn contains(&self, ip: usize) -> bool {
        ip >= self.address && ip < self.address.saturating_add(self.size)
    }

    fn file_address(&self, ip: usize) -> u64 {
        ip.saturating_sub(self.address).saturating_add(self.offset) as u64
    }

    fn candidate_addresses(&self, ip: usize) -> [u64; 3] {
        [
            self.file_address(ip),
            ip.saturating_sub(self.address) as u64,
            ip as u64,
        ]
    }

    fn object_contains(&self, address: u64) -> bool {
        self.object_ranges.is_empty()
            || self
                .object_ranges
                .iter()
                .any(|(start, end)| address >= *start && address < *end)
    }

    fn has_valid_candidate(&self, ip: usize) -> bool {
        self.candidate_addresses(ip)
            .into_iter()
            .any(|address| self.object_contains(address))
    }
}

fn object_ranges(path: &str) -> Vec<(u64, u64)> {
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(object) = object::File::parse(data.as_slice()) else {
        return Vec::new();
    };

    object
        .segments()
        .filter_map(|segment| {
            let start = segment.address();
            let size = segment.size();
            (size > 0).then_some((start, start.saturating_add(size)))
        })
        .collect()
}

pub fn resolve_proc_maps(proc_maps: &[ProcMapEntry]) -> HashMap<u32, Vec<ResolvedPME<'_>>> {
    let mut procs = HashMap::<u32, Vec<ResolvedPME>>::new();

    for pm in proc_maps {
        let vec = procs.entry(pm.pid).or_default();

        let exists = std::fs::exists(&pm.filename).unwrap_or(false);
        let loader = if exists {
            addr2line::Loader::new(&pm.filename).ok()
        } else {
            None
        };
        let object_ranges = if exists {
            object_ranges(&pm.filename)
        } else {
            Vec::new()
        };

        vec.push(ResolvedPME {
            loader,
            name: &pm.filename,
            address: pm.address,
            size: pm.size,
            offset: pm.offset,
            object_ranges,
        });
    }

    procs
}

pub fn find_sym_name(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<String> {
    pmes.iter().find_map(|entry| {
        if !entry.contains(ip) {
            return None;
        }

        let loader = entry.loader.as_ref()?;
        entry
            .candidate_addresses(ip)
            .into_iter()
            .filter(|address| entry.object_contains(*address))
            .find_map(|address| loader.find_symbol(address))
            .map(String::from)
    })
}

pub fn find_location(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<(String, u32)> {
    pmes.iter().find_map(|entry| {
        if !entry.contains(ip) {
            return None;
        }

        let loader = entry.loader.as_ref()?;
        entry
            .candidate_addresses(ip)
            .into_iter()
            .filter(|address| entry.object_contains(*address))
            .find_map(|address| loader.find_location(address).ok().flatten())
            .map(|loc| {
                (
                    loc.file.unwrap_or_default().to_string(),
                    loc.line.unwrap_or_default(),
                )
            })
    })
}

pub fn find_module_path(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<String> {
    pmes.iter()
        .find(|entry| entry.contains(ip) && entry.has_valid_candidate(ip))
        .or_else(|| pmes.iter().find(|entry| entry.contains(ip)))
        .map(|entry| entry.name.to_string())
}

#[cfg(test)]
mod tests {
    use super::{find_sym_name, resolve_proc_maps, ResolvedPME};

    #[test]
    fn rejects_aslr_runtime_addresses_outside_object_segments() {
        let entry = ResolvedPME {
            loader: None,
            name: "test",
            address: 0x1_040a_0000,
            size: 0x4000,
            offset: 0,
            object_ranges: vec![(0x1_0000_0000, 0x1_0000_4000)],
        };

        assert!(!entry.has_valid_candidate(0x1_040a_049c));
    }

    #[test]
    fn accepts_unslid_mach_o_address() {
        let entry = ResolvedPME {
            loader: None,
            name: "test",
            address: 0x1_040a_0000,
            size: 0x4000,
            offset: 0x1_0000_0000,
            object_ranges: vec![(0x1_0000_0000, 0x1_0000_4000)],
        };

        assert!(entry.has_valid_candidate(0x1_040a_049c));
    }

    #[cfg(target_os = "macos")]
    #[inline(never)]
    #[no_mangle]
    extern "C" fn miniperf_symbol_resolution_probe() {
        std::hint::black_box(());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chooses_valid_unslid_mapping_over_overlapping_zero_offset_mapping() {
        use mperf_data::ProcMapEntry;
        use object::{Object, ObjectSegment};

        extern "C" {
            static _mh_execute_header: u8;
        }

        let executable = std::env::current_exe().unwrap();
        let data = std::fs::read(&executable).unwrap();
        let object = object::File::parse(data.as_slice()).unwrap();
        let text = object
            .segments()
            .find(|segment| segment.name().ok().flatten() == Some("__TEXT"))
            .unwrap();
        let runtime_base = std::ptr::addr_of!(_mh_execute_header) as usize;
        let runtime_size = text.size() as usize;
        let link_base = text.address() as usize;
        let ip = miniperf_symbol_resolution_probe as *const () as usize;
        let filename = executable.to_string_lossy().to_string();

        let maps = vec![
            ProcMapEntry {
                filename: filename.clone(),
                address: runtime_base,
                size: runtime_size,
                offset: 0,
                pid: 1,
            },
            ProcMapEntry {
                filename,
                address: runtime_base,
                size: runtime_size,
                offset: link_base,
                pid: 1,
            },
        ];
        let resolved = resolve_proc_maps(&maps);
        let symbol = find_sym_name(resolved.get(&1).unwrap(), ip).unwrap();

        assert!(symbol.contains("miniperf_symbol_resolution_probe"));
    }
}
