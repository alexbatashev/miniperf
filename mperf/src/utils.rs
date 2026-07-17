use mperf_data::{EventType, ProcMapEntry};
use pmu::Counter;
use symbolize::{ProcessMap, Resolver};

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

pub fn resolve_proc_maps(proc_maps: &[ProcMapEntry]) -> Resolver {
    Resolver::new(proc_maps.iter().map(|map| ProcessMap {
        pid: map.pid,
        path: map.filename.clone().into(),
        start: map.address as u64,
        end: map.address.saturating_add(map.size) as u64,
        offset: map.offset as u64,
    }))
}
