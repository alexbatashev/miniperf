use clap::ValueEnum;
use serde::{Deserialize, Serialize};

mod event;
mod ipc;

pub use event::{CallFrame, Event, EventType, IString, Location, ProcMapEntry, UserRegs};
pub use ipc::{IPCMessage, IPCString};

/// Version of the on-disk results format written by this build.
///
/// This covers both the JSON metadata and the bincode event stream. Increment
/// it whenever either layout changes incompatibly.
/// Version 2 adds the raw user registers and stack bytes used for post-hoc unwinding.
pub const CURRENT_FORMAT_VERSION: u32 = 2;

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
    /// On-disk results format. Missing means the legacy, pre-versioned format.
    #[serde(default)]
    pub format_version: u32,
    pub scenario: Scenario,
    pub command: Option<Vec<String>>,
    pub cpu_model: String,
    pub cpu_vendor: String,
    /// Core clusters on a heterogeneous host (empty on homogeneous systems).
    #[serde(default)]
    pub cores: Vec<CoreCluster>,
    pub scenario_info: ScenarioInfo,
}

impl RecordInfo {
    pub fn ensure_supported_format(&self) -> Result<(), UnsupportedFormatVersion> {
        if self.format_version > CURRENT_FORMAT_VERSION {
            return Err(UnsupportedFormatVersion {
                found: self.format_version,
                supported: CURRENT_FORMAT_VERSION,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedFormatVersion {
    pub found: u32,
    pub supported: u32,
}

impl std::fmt::Display for UnsupportedFormatVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "results use format version {}, but this mperf supports up to version {}; upgrade mperf to view these results",
            self.found, self.supported
        )
    }
}

impl std::error::Error for UnsupportedFormatVersion {}

impl Scenario {
    pub fn name(&self) -> &'static str {
        match self {
            Scenario::Snapshot => "Snapshot",
            Scenario::Roofline => "Roofline",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_info_json(format_version: Option<u32>) -> String {
        let version = format_version
            .map(|version| format!(r#""format_version":{version},"#))
            .unwrap_or_default();
        format!(
            r#"{{{version}"scenario":"Snapshot","command":null,"cpu_model":"test","cpu_vendor":"test","scenario_info":{{"Snapshot":{{"pid":1,"counters":[]}}}}}}"#
        )
    }

    #[test]
    fn accepts_current_and_legacy_results() {
        for version in [None, Some(CURRENT_FORMAT_VERSION)] {
            let info: RecordInfo = serde_json::from_str(&record_info_json(version)).unwrap();
            info.ensure_supported_format().unwrap();
        }
    }

    #[test]
    fn rejects_newer_results_with_actionable_message() {
        let info: RecordInfo =
            serde_json::from_str(&record_info_json(Some(CURRENT_FORMAT_VERSION + 1))).unwrap();
        let error = info.ensure_supported_format().unwrap_err().to_string();

        assert!(error.contains("upgrade mperf"));
        assert!(error.contains(&(CURRENT_FORMAT_VERSION + 1).to_string()));
    }
}
