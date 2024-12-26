mod driver;

pub use driver::Driver;

pub enum Counter {
    Cycles,
    Instructions,
    LLCReferences,
    LLCMisses,
    BranchInstructions,
    BranchMisses,
    StalledCyclesFrontend,
    StalledCyclesBackend,
    Custom(String),
}

pub fn list_counters() -> Vec<Counter> {
    let mut counters = vec![
        Counter::Cycles,
        Counter::Instructions,
        Counter::LLCReferences,
        Counter::LLCMisses,
        Counter::BranchInstructions,
        Counter::BranchMisses,
    ];

    counters.extend(driver::list_software_counters());

    counters
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
            _ => todo!(),
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
            _ => todo!(),
        }
    }
}
