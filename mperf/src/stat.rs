use pmu::{Counter, Process};

pub fn do_stat(command: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let process = Process::new(&command)?;

    let mut driver = pmu::CountingDriver::new(
        &[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
            Counter::StalledCyclesBackend,
            Counter::StalledCyclesFrontend,
        ],
        Some(&process),
    )?;

    driver.reset()?;
    process.cont();
    process.wait()?;
    driver.stop()?;

    for cntr in driver.get_counters()? {
        println!("{}: {}", cntr.0.name(), cntr.1);
    }

    Ok(())
}
