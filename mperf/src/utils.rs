use std::collections::HashMap;

use mperf_data::{Event, EventType, ProcMap};
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
}

pub fn resolve_proc_maps(proc_maps: &[ProcMap]) -> HashMap<u32, Vec<ResolvedPME<'_>>> {
    let mut procs = HashMap::<u32, Vec<ResolvedPME>>::new();

    for pm in proc_maps {
        let entries = pm
            .entries
            .iter()
            .map(|e| {
                let loader = if std::fs::exists(&e.filename).unwrap() {
                    addr2line::Loader::new(&e.filename).ok()
                } else {
                    None
                };

                ResolvedPME {
                    loader,
                    name: &e.filename,
                    address: e.address,
                    size: e.size,
                }
            })
            .collect::<Vec<_>>();

        procs.insert(pm.pid, entries);
    }

    procs
}

pub fn find_sym_name(pmes: &[ResolvedPME<'_>], ip: usize) -> Option<String> {
    pmes.iter()
        .find_map(|entry| {
            if ip < entry.address || ip > entry.address + entry.size {
                return None;
            }

            entry
                .loader
                .as_ref()
                .and_then(|loader| loader.find_symbol((ip - entry.address) as u64))
                .map(String::from)
        })
        .or_else(|| {
            pmes[0]
                .loader
                .as_ref()
                .and_then(|loader| loader.find_symbol(ip as u64).map(String::from))
        })
}

pub fn get_event_readable_name(evt: &Event) -> String {
    match evt.ty {
        EventType::PmuCycles => "Cycles",
        EventType::PmuInstructions => "Instructions",
        EventType::PmuBranchInstructions => "Branches",
        EventType::PmuBranchMisses => "Branch misses",
        EventType::PmuLlcReferences => "LLC References",
        EventType::PmuLlcMisses => "LLC Misses",
        EventType::PmuStalledCyclesFrontend => "Stalled cycles frontend",
        EventType::PmuStalledCyclesBackend => "Stalled cycles backend",
        EventType::OsCpuClock => "CPU clock",
        EventType::OsUserTime => "User time",
        EventType::OsTotalTime => "Total time",
        EventType::OsPageFaults => "Page faults",
        EventType::OsSystemTime => "System time",
        EventType::OsCpuMigrations => "CPU migrations",
        EventType::OsContextSwitches => "Context switches",
        _ => unimplemented!(),
    }
    .to_string()
}
