use core::fmt;
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
    pub callstack: SmallVec<[CallFrame; 32]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct ProcMapEntry {
    pub filename: String,
    pub address: usize,
    pub size: usize,
    pub offset: usize,
    pub pid: u32,
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

    pub fn is_roofline(&self) -> bool {
        *self == EventType::RooflineLoopStart
            || *self == EventType::RooflineLoopEnd
            || *self == EventType::RooflineBytesLoad
            || *self == EventType::RooflineBytesStore
            || *self == EventType::RooflineScalarIntOps
            || *self == EventType::RooflineScalarFloatOps
            || *self == EventType::RooflineScalarDoubleOps
            || *self == EventType::RooflineVectorIntOps
            || *self == EventType::RooflineVectorFloatOps
            || *self == EventType::RooflineVectorDoubleOps
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::PmuCycles => f.write_str("pmu_cycles"),
            EventType::PmuInstructions => f.write_str("pmu_instructions"),
            EventType::PmuLlcReferences => f.write_str("pmu_llc_references"),
            EventType::PmuLlcMisses => f.write_str("pmu_llc_misses"),
            EventType::PmuBranchInstructions => f.write_str("pmu_branch_instructions"),
            EventType::PmuBranchMisses => f.write_str("pmu_branch_misses"),
            EventType::PmuStalledCyclesFrontend => f.write_str("pmu_stalled_cycles_frontend"),
            EventType::PmuStalledCyclesBackend => f.write_str("pmu_stalled_cycles_backend"),
            EventType::PmuCustom => f.write_str("pmu_unknown"),
            EventType::OsCpuClock => f.write_str("os_cpu_clock"),
            EventType::OsCpuMigrations => f.write_str("os_cpu_migrations"),
            EventType::OsPageFaults => f.write_str("os_page_faults"),
            EventType::OsContextSwitches => f.write_str("os_context_switches"),
            EventType::OsTotalTime => f.write_str("os_total_time"),
            EventType::OsUserTime => f.write_str("os_user_time"),
            EventType::OsSystemTime => f.write_str("os_system_time"),
            EventType::RooflineBytesLoad => f.write_str("roofline_bytes_load"),
            EventType::RooflineBytesStore => f.write_str("roofline_bytes_store"),
            EventType::RooflineScalarIntOps => f.write_str("roofline_scalar_int_ops"),
            EventType::RooflineScalarFloatOps => f.write_str("roofline_scalar_float_ops"),
            EventType::RooflineScalarDoubleOps => f.write_str("roofline_scalar_double_ops"),
            EventType::RooflineVectorIntOps => f.write_str("roofline_vector_int_ops"),
            EventType::RooflineVectorFloatOps => f.write_str("roofline_vector_float_ops"),
            EventType::RooflineVectorDoubleOps => f.write_str("roofline_vector_double_ops"),
            EventType::RooflineLoopStart => f.write_str("roofline_loop_start"),
            EventType::RooflineLoopEnd => f.write_str("roofline_loop_end"),
        }
    }
}

impl CallFrame {
    pub fn as_ip(&self) -> u64 {
        match self {
            CallFrame::IP(ip) => *ip,
            CallFrame::Location(_) => unreachable!(),
        }
    }
    pub fn as_loc(&self) -> Location {
        match self {
            CallFrame::Location(loc) => loc.clone(),
            CallFrame::IP(_) => unreachable!(),
        }
    }
}
