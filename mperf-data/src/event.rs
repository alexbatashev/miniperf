use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IString {
    pub id: u64,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum EventType {
    PmuCycles,
    PmuInstructions,
    PmuLlcReferences,
    PmuLlcMisses,
    PmuBranchInstructions,
    PmuBranchMisses,
    PmuStalledCyclesFrontend,
    PmuStalledCyclesBackend,
    PmuCustom,
    OsCpuClock,
    OsCpuMigrations,
    OsPageFaults,
    OsContextSwitches,
    OsTotalTime,
    OsUserTime,
    OsSystemTime,
    RooflineBytesLoad,
    RooflineBytesStore,
    RooflineScalarIntOps,
    RooflineScalarFloatOps,
    RooflineScalarDoubleOps,
    RooflineVectorIntOps,
    RooflineVectorFloatOps,
    RooflineVectorDoubleOps,
    RooflineLoopStart,
    RooflineLoopEnd,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[repr(C)]
pub struct Event {
    pub unique_id: u128,
    pub correlation_id: u128,
    pub parent_id: u128,
    pub ty: EventType,
    pub thread_id: u32,
    pub process_id: u32,
    pub time_enabled: u64,
    pub time_running: u64,
    pub value: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcMapEntry {
    pub filename: String,
    pub address: usize,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcMap {
    pub pid: u32,
    pub entries: Vec<ProcMapEntry>,
}
