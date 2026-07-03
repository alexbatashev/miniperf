mod cpu_family;
mod driver;
mod process;

pub use cpu_family::host_cpu_description;
pub use driver::{
    list_supported_counters, CoreId, CounterEntry, CounterResult, CounterValue, CountingDriver,
    CountingDriverBuilder, DriverKind, Record, SamplingDriver, SamplingDriverBuilder,
};
pub use process::Process;

/// The core clusters present on the host, on a heterogeneous (big.LITTLE)
/// system. Returns an empty vector on homogeneous systems (a single cluster),
/// where per-core attribution is meaningless.
pub fn host_core_clusters() -> Vec<CoreId> {
    let pmus = cpu_family::host_core_pmus();
    if pmus.len() <= 1 {
        return Vec::new();
    }

    pmus.iter()
        .map(|pmu| {
            let name = cpu_family::find_cpu_family(pmu.family_id)
                .map(|f| f.name.clone())
                .unwrap_or_else(|| pmu.family_id.to_string());
            CoreId {
                family_id: pmu.family_id.to_string(),
                name,
                cpus: pmu.cpus.clone(),
            }
        })
        .collect()
}

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

    pub fn is_software(&self) -> bool {
        match self {
            Counter::CpuClock
            | Counter::PageFaults
            | Counter::ContextSwitches
            | Counter::CpuMigrations => true,
            _ => false,
        }
    }
}
