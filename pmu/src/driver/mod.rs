#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "macos")]
pub use kperf::{Driver, list_software_counters};

