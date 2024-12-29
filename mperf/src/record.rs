use pmu::Counter;

use crate::Scenario;

pub fn do_record(
    scenario: Scenario,
    _output_directory: String,
    _pid: Option<usize>,
    _command: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Record profile with {scenario:?}");

    let _driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
        ])
        .build()?;

    Ok(())
}