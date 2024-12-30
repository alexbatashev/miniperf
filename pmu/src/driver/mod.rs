#[cfg(target_os = "linux")]
mod perf;

#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "linux")]
pub use perf::{list_software_counters, CountingDriver, SamplingDriver};

#[cfg(target_os = "macos")]
pub use kperf::{list_software_counters, CountingDriver, SamplingDriver};

/// A structure that represents a single sample
#[derive(Debug)]
pub struct Sample {
    /// Unique ID shared by all samples of the event
    pub event_id: u64,
    /// Instruction pointer
    pub ip: u64,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
    /// Timestamp
    pub time: u64,
    pub time_enabled: u64,
    pub time_running: u64,
    pub counter: crate::Counter,
    pub value: u64,
}
