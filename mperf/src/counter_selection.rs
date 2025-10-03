use mperf_data::Scenario;
use pmu::Counter;

pub fn get_pmu_counters(scenario: Scenario) -> Vec<Counter> {
    match scenario {
        Scenario::Snapshot | Scenario::Roofline => vec![
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
            Counter::StalledCyclesBackend,
            Counter::StalledCyclesFrontend,
            Counter::CpuClock,
            Counter::CpuMigrations,
            Counter::PageFaults,
            Counter::ContextSwitches,
        ],
        Scenario::TMA => pmu::host_tma_scenario()
            .expect("TMA counter selection requires a supported host CPU")
            .events
            .iter()
            .map(|evt| match evt.as_str() {
                "cycles" => Counter::Cycles,
                "instructions" => Counter::Instructions,
                _ => Counter::Custom(evt.clone()),
            })
            .chain(
                [
                    Counter::CpuClock,
                    Counter::CpuMigrations,
                    Counter::PageFaults,
                    Counter::ContextSwitches,
                ]
                .into_iter(),
            )
            .collect(),
    }
}
