#[cfg(target_os = "linux")]
mod perf;

#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "linux")]
pub use perf::{list_supported_counters, CountingDriver, Record, SamplingDriver};

#[cfg(target_os = "macos")]
pub use kperf::{list_software_counters, Driver};
