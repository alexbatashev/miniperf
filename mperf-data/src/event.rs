use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
#[repr(u64)]
pub enum RooflineEventId {
    BytesLoad,
    BytesStore,
    ScalarIntOps,
    ScalarFloatOps,
    VectorIntOps,
    VectorFloatOps,
    VectorDoubleOps,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IString {
    pub id: u64,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum EventType {
    PMU,
    LoopStart,
    LoopEnd,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[repr(C)]
pub struct Event {
    pub unique_id: u64,
    pub correlation_id: u64,
    pub parent_id: u64,
    pub ty: EventType,
    pub name: u64,
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
