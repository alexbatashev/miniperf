use std::io::{BufRead, Write};

use capnp::message::ReaderOptions;
use serde::{Deserialize, Serialize};

use crate::event_capnp;

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
