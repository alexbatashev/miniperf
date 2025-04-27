use std::collections::HashMap;

use mperf_data::{EventType, ProcMapEntry};
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
}

pub fn resolve_proc_maps(proc_maps: &[ProcMapEntry]) -> HashMap<u32, Vec<ResolvedPME<'_>>> {
    let mut procs = HashMap::<u32, Vec<ResolvedPME>>::new();

    for pm in proc_maps {
        let vec = procs.entry(pm.pid).or_default();

        let loader = if std::fs::exists(&pm.filename).unwrap() {
            addr2line::Loader::new(&pm.filename).ok()
        } else {
            None
        };

        vec.push(ResolvedPME {
            loader,
            name: &pm.filename,
            address: pm.address,
            size: pm.size,
            offset: pm.offset,
        });
    }

    procs
}

pub fn find_sym_name(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<String> {
    pmes.iter().find_map(|entry| {
        if ip < entry.address || ip > entry.address + entry.size {
            return None;
        }

        entry
            .loader
            .as_ref()
            .and_then(|loader| loader.find_symbol(ip as u64).or(loader.find_symbol((ip - entry.address) as u64)))
            .map(String::from)
    })
}

pub fn find_location(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<(String, u32)> {
    pmes.iter().find_map(|entry| {
        if ip < entry.address || ip > entry.address + entry.size {
            return None;
        }

        let file_addr = ip - entry.address + entry.offset;

        entry
            .loader
            .as_ref()
            .and_then(|loader| loader.find_location(file_addr as u64).ok())
            .flatten()
            .map(|loc| {
                (
                    loc.file.unwrap_or_default().to_string(),
                    loc.line.unwrap_or_default(),
                )
            })
    })
}
