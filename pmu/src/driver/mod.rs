#[cfg(target_os = "linux")]
mod perf;

#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "linux")]
use perf::{PerfCountingDriver, PerfSamplingDriver};

#[cfg(target_os = "macos")]
pub use kperf::{list_software_counters, Driver};

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

#[derive(Debug, Clone)]
pub struct CounterResult {
    values: SmallVec<[(Counter, CounterValue); 16]>,
}

/// Sampling driver produces records that describe events
#[derive(Debug)]
pub enum Record {
    Sample(Sample),
    ProcAddr(ProcAddr),
}
=======
pub use kperf::{list_software_counters, CountingDriver, SamplingDriver};
>>>>>>> 9b74494 (save progress)

/// A structure that represents a single sample
#[derive(Debug)]
pub struct Sample {
    /// Unique ID shared by all samples of the event
<<<<<<< HEAD
    pub event_id: u128,
=======
    pub event_id: u64,
>>>>>>> 9b74494 (save progress)
    /// Instruction pointer
    pub ip: u64,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
<<<<<<< HEAD
    /// CPU ID that the event occured on
    pub cpu: u32,
=======
>>>>>>> 9b74494 (save progress)
    /// Timestamp
    pub time: u64,
    pub time_enabled: u64,
    pub time_running: u64,
<<<<<<< HEAD
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

    pub fn build(self) -> Result<Box<dyn CountingDriver>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os="linux")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::Perf {
                    return Ok(Box::new(PerfCountingDriver::new(self.counters, self.pid)?));
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
            }
        }

        unimplemented!()
    }
}

impl CounterResult {
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        self.values
            .iter()
            .find(|(c, _)| *c == kind)
            .map(|(_, v)| v)
            .cloned()
    }
}

impl IntoIterator for CounterResult {
    type Item = (Counter, CounterValue);

    type IntoIter = <SmallVec<[(Counter, CounterValue); 16]> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}
