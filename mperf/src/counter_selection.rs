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
            Counter::CpuClock,
            Counter::CpuMigrations,
            Counter::PageFaults,
            Counter::ContextSwitches,
        ],
        Scenario::TMA => {
            let family_name = pmu::cpu_family::get_host_cpu_family();
            let family = pmu::cpu_family::find_cpu_family(family_name);

            if family.is_none() {
                unimplemented!()
            }

            let family = family.unwrap();

            if !family.scenarios.contains_key("tma") {
                unimplemented!()
            }

            family
                .scenarios
                .get("tma")
                .unwrap()
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
                .collect()
        }
    }
}
