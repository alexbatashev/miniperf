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

/// A cluster of cores on a heterogeneous (big.LITTLE) system, used to attribute
/// samples to a specific core type during post-processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreCluster {
    /// Family id, e.g. `"cortex_a720"`.
    pub family_id: String,
    /// Human readable name, e.g. `"ARM Cortex-A720"`.
    pub name: String,
    /// sysfs cpumask string, e.g. `"0,5-11"`.
    pub cpus: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordInfo {
    pub scenario: Scenario,
    pub command: Option<Vec<String>>,
    pub cpu_model: String,
    pub cpu_vendor: String,
    /// Core clusters on a heterogeneous host (empty on homogeneous systems).
    #[serde(default)]
    pub cores: Vec<CoreCluster>,
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
