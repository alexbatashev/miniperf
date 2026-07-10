use mperf_data::{EventType, ProcMapEntry};
use pmu::Counter;
use symbolize::{Frame, ProcessMap, Resolver};

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

pub fn find_frames(resolver: &Resolver, pid: u32, ip: usize) -> Vec<Frame> {
    resolver.resolve(pid, ip as u64)
}

pub fn find_sym_name(resolver: &Resolver, pid: u32, ip: usize) -> Option<String> {
    find_frames(resolver, pid, ip)
        .into_iter()
        .next()
        .map(|frame| frame.function)
}

pub fn find_location(resolver: &Resolver, pid: u32, ip: usize) -> Option<(String, u32)> {
    find_frames(resolver, pid, ip)
        .into_iter()
        .next()
        .map(|frame| {
            (
                frame.file.unwrap_or_default(),
                frame.line.unwrap_or_default(),
            )
        })
}

pub fn find_module_path(resolver: &Resolver, pid: u32, ip: usize) -> Option<String> {
    resolver
        .module_path(pid, ip as u64)
        .map(|path| path.to_string_lossy().into_owned())
}
