#[cfg(target_os = "linux")]
mod perf;

#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "linux")]
use perf::{PerfCountingDriver, PerfSamplingDriver};

#[cfg(target_os = "macos")]
use kperf::{KPerfCountingDriver, KPerfSamplingDriver};

use itertools::chain;
use smallvec::SmallVec;
use std::sync::Arc;

use crate::{cpu_family, Counter, Error, Process};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Default,
    Perf,
    KPerf,
}

#[derive(Debug, Clone)]
pub struct CounterValue {
    pub value: u64,
    pub scaling: f64,
    pub quality: MeasurementQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementQuality {
    Exact,
    Scaled,
    Estimated,
}

/// Counting driver is used for simple collection of system's performance counters values. On Linux,
/// counter multiplexing is supported.
pub trait CountingDriver {
    fn start(&mut self) -> Result<(), Error>;
    fn stop(&mut self) -> Result<(), Error>;
    fn reset(&mut self) -> Result<(), Error>;
    fn counters(&mut self) -> Result<CounterResult, std::io::Error>;
}

pub trait SamplingCallback: Send + Sync {
    fn call(&self, record: Record);
}

pub trait SamplingDriver {
    fn start(&mut self, callback: Arc<dyn SamplingCallback>) -> Result<(), Error>;

    fn stop(&mut self) -> Result<(), Error>;
}

/// Identifies the core cluster a counter value was measured on, on a
/// heterogeneous (big.LITTLE) system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreId {
    /// Family id, e.g. `"cortex_a720"`.
    pub family_id: String,
    /// Human readable name, e.g. `"ARM Cortex-A720"`.
    pub name: String,
    /// sysfs cpumask for the cluster, e.g. `"0,5-11"`.
    pub cpus: String,
}

/// A single measured counter value, tagged with the core it was measured on.
#[derive(Debug, Clone)]
pub struct CounterEntry {
    /// The core cluster this value came from. `None` on homogeneous systems and
    /// for software counters, which are not PMU-specific.
    pub core: Option<CoreId>,
    pub counter: Counter,
    pub value: CounterValue,
}

#[derive(Debug, Clone, Default)]
pub struct CounterResult {
    entries: SmallVec<[CounterEntry; 16]>,
}

/// Sampling driver produces records that describe events
#[derive(Debug)]
pub enum Record {
    Sample(Sample),
    ProcAddr(ProcAddr),
}

/// A structure that represents a single sample
#[derive(Debug)]
pub struct Sample {
    /// Unique ID shared by all samples of the event
    pub event_id: u128,
    /// Instruction pointer
    pub ip: u64,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
    /// CPU ID that the event occured on
    pub cpu: u32,
    /// Family id of the core cluster this sample came from (e.g.
    /// `"cortex_a720"`), on a heterogeneous system. `None` on homogeneous hosts.
    pub core: Option<String>,
    /// Timestamp
    pub time: u64,
    pub time_enabled: u64,
    pub time_running: u64,
    pub counter: Counter,
    pub value: u64,
    pub callstack: SmallVec<[u64; 32]>,
}

#[derive(Debug)]
pub struct ProcAddr {
    pub pid: u32,
    pub addr: u64,
    pub len: u64,
    pub pgoff: u64,
    pub filename: String,
}

pub struct CountingDriverBuilder {
    counters: Vec<Counter>,
    pid: Option<i32>,
    kind: DriverKind,
}

pub struct SamplingDriverBuilder {
    counters: Vec<Counter>,
    sample_freq: u64,
    pid: Option<i32>,
    prefer_raw_events: bool,
    kind: DriverKind,
}

impl<F: Fn(Record) + Send + Sync> SamplingCallback for F {
    fn call(&self, record: Record) {
        self(record)
    }
}

pub fn list_supported_counters(driver: DriverKind) -> Vec<Counter> {
    cfg_if::cfg_if! {
        if #[cfg(target_os="linux")] {
            if driver == DriverKind::Default || driver == DriverKind::Perf {
                return perf::list_supported_counters();
            }
        }
    }

    cfg_if::cfg_if! {
        if #[cfg(target_os="macos")] {
            if driver == DriverKind::Default || driver == DriverKind::KPerf {
                return kperf::list_supported_counters();
            }
        }
    }

    vec![]
}

impl CountingDriverBuilder {
    pub fn new() -> Self {
        CountingDriverBuilder {
            counters: vec![],
            pid: None,
            kind: DriverKind::Default,
        }
    }

    pub fn counters(mut self, counters: &[Counter]) -> Self {
        self.counters = counters.to_vec();
        self
    }

    pub fn process(mut self, process: Option<&Process>) -> Self {
        self.pid = process.map(|p| p.pid());
        self
    }

    pub fn pid(mut self, pid: Option<i32>) -> Self {
        self.pid = pid;
        self
    }

    pub fn build(self) -> Result<Box<dyn CountingDriver>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os="linux")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::Perf {
                    return Ok(Box::new(PerfCountingDriver::new(self.counters, self.pid)?));
                }
            } else if #[cfg(target_os="macos")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::KPerf {
                    return Ok(Box::new(KPerfCountingDriver::new(self.counters, self.pid)?));
                }
            }
        }

        todo!()
    }
}

impl SamplingDriverBuilder {
    pub fn new() -> Self {
        SamplingDriverBuilder {
            counters: vec![],
            sample_freq: 1000,
            pid: None,
            prefer_raw_events: true,
            kind: DriverKind::Default,
        }
    }

    pub fn counters(mut self, counters: &[Counter]) -> Self {
        let cpu_family = cpu_family::get_host_cpu_family();
        let info = cpu_family::find_cpu_family(cpu_family);

        let leader = info.and_then(|info| info.leader_event.clone());

        let counters = if leader.is_some() {
            chain([Counter::Custom(leader.unwrap())], counters.iter().cloned()).collect()
        } else {
            counters.to_vec()
        };

        self.counters = counters;
        self
    }

    pub fn process(mut self, process: &Process) -> Self {
        self.pid = Some(process.pid());
        self
    }

    pub fn pid(mut self, pid: i32) -> Self {
        self.pid = Some(pid);
        self
    }

    pub fn sample_freq(mut self, sample_freq: u64) -> Self {
        self.sample_freq = sample_freq;
        self
    }

    pub fn prefer_raw_events(mut self) -> Self {
        self.prefer_raw_events = true;
        self
    }

    pub fn build(self) -> Result<Box<dyn SamplingDriver>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os="linux")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::Perf {
                    return Ok(Box::new(PerfSamplingDriver::new(&self.counters, self.sample_freq, self.pid, self.prefer_raw_events)?));
                }
            } else if #[cfg(target_os="macos")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::KPerf {
                    return Ok(Box::new(KPerfSamplingDriver::new(&self.counters, self.sample_freq, self.pid)?));
                }
            }
        }

        unimplemented!()
    }
}

impl CounterResult {
    pub fn from_entries(entries: SmallVec<[CounterEntry; 16]>) -> Self {
        CounterResult { entries }
    }

    /// Faithful total for a counter, summed across every core it was measured
    /// on. On a homogeneous system this is simply the single value.
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        let matching: SmallVec<[&CounterEntry; 8]> =
            self.entries.iter().filter(|e| e.counter == kind).collect();

        if matching.is_empty() {
            return None;
        }

        let value = matching.iter().map(|e| e.value.value).sum();
        let scaling = matching.iter().map(|e| e.value.scaling).sum::<f64>() / matching.len() as f64;

        let quality = if matching
            .iter()
            .any(|entry| entry.value.quality == MeasurementQuality::Estimated)
        {
            MeasurementQuality::Estimated
        } else if matching
            .iter()
            .any(|entry| entry.value.quality == MeasurementQuality::Scaled)
        {
            MeasurementQuality::Scaled
        } else {
            MeasurementQuality::Exact
        };

        Some(CounterValue {
            value,
            scaling,
            quality,
        })
    }

    /// Value of a counter on one specific core.
    pub fn get_for(&self, core: &Option<CoreId>, kind: Counter) -> Option<CounterValue> {
        self.entries
            .iter()
            .find(|e| e.core == *core && e.counter == kind)
            .map(|e| e.value.clone())
    }

    /// The distinct cores present, in first-seen order. Empty on homogeneous
    /// systems (all entries are untagged).
    pub fn cores(&self) -> Vec<CoreId> {
        let mut cores: Vec<CoreId> = Vec::new();
        for entry in &self.entries {
            if let Some(core) = &entry.core {
                if !cores.contains(core) {
                    cores.push(core.clone());
                }
            }
        }
        cores
    }

    pub fn entries(&self) -> &[CounterEntry] {
        &self.entries
    }
}

impl IntoIterator for CounterResult {
    type Item = CounterEntry;

    type IntoIter = <SmallVec<[CounterEntry; 16]> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
