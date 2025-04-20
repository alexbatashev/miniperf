mod cpu_family;
mod driver;
mod process;

pub use driver::{list_supported_counters, CountingDriver, Record, SamplingDriver};
pub use process::Process;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Counter {
    Cycles,
    Instructions,
    LLCReferences,
    LLCMisses,
    BranchInstructions,
    BranchMisses,
    StalledCyclesFrontend,
    StalledCyclesBackend,
    CpuClock,
    PageFaults,
    ContextSwitches,
    CpuMigrations,
    Custom(String),
    Internal {
        name: String,
        desc: String,
        code: u64,
    },
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to create counters")]
    CounterCreationFail,
    #[error("Failed to enable counters")]
    EnableFailed,
}

impl Counter {
    pub fn name(&self) -> &str {
        match self {
            Counter::Cycles => "cycles",
            Counter::Instructions => "instructions",
            Counter::LLCReferences => "llc_references",
            Counter::LLCMisses => "llc_misses",
            Counter::BranchInstructions => "branches",
            Counter::BranchMisses => "branch_misses",
            Counter::StalledCyclesFrontend => "stalled_cycles_frontend",
            Counter::StalledCyclesBackend => "stalled_cycles_backend",
            Counter::CpuClock => "cpu_clock",
            Counter::PageFaults => "page_faults",
            Counter::ContextSwitches => "context_switches",
            Counter::CpuMigrations => "cpu_migrations",
            Counter::Custom(name) => name,
            Counter::Internal {
                name,
                desc: _,
                code: _,
            } => name,
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Counter::Cycles => "Number of CPU cycles",
            Counter::Instructions => "Number of instructions retired",
            Counter::LLCReferences => "Last level cache references",
            Counter::LLCMisses => "Last level cache misses",
            Counter::BranchInstructions => "Branch instructions retired",
            Counter::BranchMisses => "Branch instruction missess",
            Counter::StalledCyclesFrontend => {
                "Number of cycles stalled due to frontend bottlenecks"
            }
            Counter::StalledCyclesBackend => "Number of cycles stalled due to backend bottlenecks",
            Counter::CpuClock => "A high-resolution per-CPU timer",
            Counter::PageFaults => "Number of page faults",
            Counter::ContextSwitches => "Number of context switches",
            Counter::CpuMigrations => "Number of the times the process has migrated to a new CPU",
            Counter::Custom(_) => "",
            Counter::Internal {
                name: _,
                desc,
                code: _,
            } => desc,
        }
    }
}
