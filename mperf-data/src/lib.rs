use clap::ValueEnum;
use pmu_data::{ScenarioUi, TmaConstant, TmaGroup, TmaMetric};
use serde::{Deserialize, Serialize};

mod event;
mod ipc;
mod ui;

pub use event::{CallFrame, Event, EventType, IString, Location, ProcMapEntry, UserRegs};
pub use ipc::{IPCMessage, IPCString};
pub use ui::scenario_ui;

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
    TMA,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub pid: i32,
    pub counters: Vec<(EventType, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RooflineInfo {
    #[serde(default = "default_roofline_backend")]
    pub backend: String,
    pub perf_pid: i32,
    pub counters: Vec<(EventType, String)>,
    pub inst_pid: i32,
}

fn default_roofline_backend() -> String {
    "compiler".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TMAInfo {
    pub pid: i32,
    pub counters: Vec<(EventType, String)>,
    #[serde(default)]
    pub groups: Vec<TmaGroup>,
    /// True only when the recording used a precise PMU sampling source.
    #[serde(default)]
    pub precise_attribution: bool,
    pub metrics: Vec<TmaMetric>,
    pub constants: Vec<TmaConstant>,
    #[serde(default)]
    pub ui: Option<ScenarioUi>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioInfo {
    Snapshot(SnapshotInfo),
    Roofline(RooflineInfo),
    TMA(TMAInfo),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Host ceilings measured immediately before a Roofline recording.
    #[serde(default)]
    pub roofline_calibration: Option<Box<RooflineCalibration>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RooflineCalibration {
    /// Number of Rayon workers used by both calibration kernels.
    pub threads: usize,
    /// Host CPUs on which the process was allowed to run, when available.
    #[serde(default)]
    pub cpu_affinity: Option<String>,
    /// Samples contributing to each reported median.
    pub samples: usize,
    /// Name of the architecture-specific FP64 compute kernel.
    pub compute_kernel: String,
    /// Median aggregate FP64 throughput.
    pub fp64_gflops: f64,
    /// Individual aggregate FP64 throughput samples.
    #[serde(default)]
    pub fp64_gflops_samples: Vec<f64>,
    /// Median aggregate triad bandwidth.
    pub memory_gbytes_per_second: f64,
    /// Individual aggregate triad-bandwidth samples.
    #[serde(default)]
    pub memory_gbytes_per_second_samples: Vec<f64>,
    /// Compute/bandwidth intersection.
    pub ridge_point_flops_per_byte: f64,
    /// Bytes in all three buffers used by the memory calibration.
    pub memory_working_set_bytes: u64,
}

/// How `os_cpu_clock` observations should be interpreted by time-based views.
///
/// A counter delta is paired with an explicit wall-time interval. Sampled
/// occupancy attributes a CPU-time weight to the logical CPU reported at the
/// sample endpoint: Darwin kperf emits a fixed timer period, while Linux perf
/// emits a task-clock delta from one authoritative sampling group. Endpoint
/// attribution is statistical when a task may have migrated within the delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuClockSource {
    CounterDelta,
    SampledOccupancy,
}

impl CpuClockSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CounterDelta => "counter_delta",
            Self::SampledOccupancy => "sampled_occupancy",
        }
    }
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
    /// Requested profiler sampling frequency. Missing on recordings written
    /// before temporal sample metadata was persisted.
    #[serde(default)]
    pub sampling_frequency_hz: Option<u64>,
    /// Semantics of timestamped CPU-clock observations. Missing on recordings
    /// created before CPU-lane data was preserved independently.
    #[serde(default)]
    pub cpu_clock_source: Option<CpuClockSource>,
    /// Number of logical CPUs in the source host topology. Current recordings
    /// use contiguous logical CPU IDs `0..logical_cpu_count`; missing means the
    /// recording predates topology preservation.
    #[serde(default)]
    pub logical_cpu_count: Option<u32>,
    /// Core clusters on a heterogeneous host (empty on homogeneous systems).
    #[serde(default)]
    pub cores: Vec<CoreCluster>,
    /// Extensible host CPU measurements collected with this recording.
    #[serde(default)]
    pub cpu_info: CpuInfo,
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
            Scenario::TMA => "Top-Down",
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
            assert!(info.cpu_info.roofline_calibration.is_none());
            assert_eq!(info.cpu_clock_source, None);
            assert_eq!(info.logical_cpu_count, None);
        }
    }

    #[test]
    fn cpu_clock_source_has_stable_machine_readable_names() {
        assert_eq!(
            serde_json::to_string(&CpuClockSource::CounterDelta).unwrap(),
            "\"counter_delta\""
        );
        assert_eq!(
            serde_json::to_string(&CpuClockSource::SampledOccupancy).unwrap(),
            "\"sampled_occupancy\""
        );
    }

    #[test]
    fn rejects_newer_results_with_actionable_message() {
        let info: RecordInfo =
            serde_json::from_str(&record_info_json(Some(CURRENT_FORMAT_VERSION + 1))).unwrap();
        let error = info.ensure_supported_format().unwrap_err().to_string();

        assert!(error.contains("upgrade mperf"));
        assert!(error.contains(&(CURRENT_FORMAT_VERSION + 1).to_string()));
    }

    #[test]
    fn round_trips_roofline_calibration_in_cpu_info() {
        let calibration = RooflineCalibration {
            threads: 4,
            cpu_affinity: Some("0-3".to_string()),
            samples: 5,
            compute_kernel: "test-fma".to_string(),
            fp64_gflops: 100.0,
            fp64_gflops_samples: vec![100.0],
            memory_gbytes_per_second: 50.0,
            memory_gbytes_per_second_samples: vec![50.0],
            ridge_point_flops_per_byte: 2.0,
            memory_working_set_bytes: 1024,
        };
        let cpu_info = CpuInfo {
            roofline_calibration: Some(Box::new(calibration)),
        };

        let json = serde_json::to_string(&cpu_info).unwrap();
        let decoded: CpuInfo = serde_json::from_str(&json).unwrap();
        let decoded = decoded.roofline_calibration.unwrap();

        assert_eq!(decoded.threads, 4);
        assert_eq!(decoded.cpu_affinity.as_deref(), Some("0-3"));
        assert_eq!(decoded.compute_kernel, "test-fma");
        assert_eq!(decoded.fp64_gflops_samples, vec![100.0]);
        assert_eq!(decoded.memory_gbytes_per_second_samples, vec![50.0]);
        assert_eq!(decoded.ridge_point_flops_per_byte, 2.0);
    }
}
