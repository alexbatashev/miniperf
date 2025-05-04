use clap::ValueEnum;
use pmu_data::{Constant, Metric};
use serde::{Deserialize, Serialize};

mod event;
mod ipc;

pub use event::{CallFrame, Event, EventType, IString, Location, ProcMapEntry};
pub use ipc::{IPCMessage, IPCString};

#[derive(Clone, Debug, Copy, ValueEnum, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scenario {
    Snapshot,
    Roofline,
    TMA,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub pid: i32,
    pub counters: Vec<(EventType, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RooflineInfo {
    pub perf_pid: i32,
    pub counters: Vec<(EventType, String)>,
    pub inst_pid: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TMAInfo {
    pub pid: i32,
    pub counters: Vec<(EventType, String)>,
    pub metrics: Vec<Metric>,
    pub constants: Vec<Constant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioInfo {
    Snapshot(SnapshotInfo),
    Roofline(RooflineInfo),
    TMA(TMAInfo),
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
            Scenario::TMA => "Top-Down",
        }
    }
}
