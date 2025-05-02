use clap::ValueEnum;
use serde::{Deserialize, Serialize};

mod event;
mod ipc;

pub use event::{CallFrame, Event, EventType, IString, Location, ProcMapEntry};
pub use ipc::{IPCMessage, IPCString};

#[derive(Clone, Debug, Copy, ValueEnum, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scenario {
    Snapshot,
    Roofline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub pid: i32,
    pub counters: Vec<EventType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RooflineInfo {
    pub perf_pid: i32,
    pub counters: Vec<EventType>,
    pub inst_pid: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioInfo {
    Snapshot(SnapshotInfo),
    Roofline(RooflineInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordInfo {
    pub scenario: Scenario,
    pub command: Option<Vec<String>>,
    pub cpu_model: String,
    pub cpu_vendor: String,
    pub scenario_info: ScenarioInfo,
}

impl Scenario {
    pub fn name(&self) -> &'static str {
        match self {
            Scenario::Snapshot => "Snapshot",
            Scenario::Roofline => "Roofline",
        }
    }
}
