use std::{thread, time::Duration};

use pmu::Counter;

use crate::Scenario;

pub fn do_record(
    scenario: Scenario,
    _output_directory: String,
    _pid: Option<usize>,
    _command: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Record profile with {scenario:?}");

    let driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
        ])
        .build()?;

    driver.start(|sample| {
        println!("got sample {:?}", sample);
    })?;
    thread::sleep(Duration::from_secs(1));
    driver.stop()?;

    Ok(())
}
