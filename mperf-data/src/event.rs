use std::io::{BufRead, Write};

use capnp::message::ReaderOptions;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::event_capnp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IString {
    pub id: u64,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Location {
    pub function_name: u128,
    pub file_name: u128,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CallFrame {
    Location(Location),
    IP(u64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub ip: u64,
    pub callstack: SmallVec<[CallFrame; 32]>,
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

impl Event {
    pub fn write_binary(&self, writer: &mut dyn Write) -> Result<(), Box<dyn std::error::Error>> {
        use capnp::serialize_packed;
        let mut message = capnp::message::Builder::new_default();
        let mut event = message.init_root::<event_capnp::event::Builder>();
        event.set_event(self);

        serialize_packed::write_message(writer, &message).map_err(|e| e.into())
    }

    pub fn read_binary(reader: &mut dyn BufRead) -> Result<Self, Box<dyn std::error::Error>> {
        use capnp::serialize_packed;

        let mut buf = [0_u8; 2 * std::mem::size_of::<Self>()];

        let message =
            serialize_packed::read_message_no_alloc(reader, &mut buf, ReaderOptions::default())?;

        let root = message.get_root::<event_capnp::event::Reader>()?;

        Ok(root.into())
    }
}

impl EventType {
    pub fn is_pmu(&self) -> bool {
        *self == EventType::PmuCycles
            || *self == EventType::PmuInstructions
            || *self == EventType::PmuBranchInstructions
            || *self == EventType::PmuBranchMisses
            || *self == EventType::PmuLlcReferences
            || *self == EventType::PmuLlcMisses
            || *self == EventType::PmuStalledCyclesBackend
            || *self == EventType::PmuStalledCyclesFrontend
            || *self == EventType::PmuCustom
    }

    pub fn is_os(&self) -> bool {
        *self == EventType::OsContextSwitches
            || *self == EventType::OsCpuMigrations
            || *self == EventType::OsSystemTime
            || *self == EventType::OsPageFaults
            || *self == EventType::OsTotalTime
            || *self == EventType::OsUserTime
            || *self == EventType::OsCpuClock
    }
}

impl ProcMap {
    pub fn new(map: (u32, Vec<ProcMapEntry>)) -> Self {
        Self {
            pid: map.0,
            entries: map.1,
        }
    }
}
