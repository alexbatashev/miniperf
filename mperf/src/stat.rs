use comfy_table::{Cell, CellAlignment, Table};
use num_format::{Locale, ToFormattedString};
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

    let mut table = Table::new();
    table.set_header(vec!["Counter", "Value", "Info", "Scaling", "Description"]);

    let result = driver.counters()?;
    for (cntr, value) in result.clone() {
        let value_str = match cntr {
            Counter::Instructions | Counter::BranchInstructions => {
                let cycles = result.get(Counter::Cycles).unwrap().value;
                let instrs = value.value;
                format!("{:.2} inst/cycle", instrs as f64 / cycles as f64)
            }
            Counter::BranchMisses | Counter::LLCMisses => {
                let instrs = result.get(Counter::Instructions).unwrap().value;
                let misses = value.value;
                format!("{:.2} MPKI", misses as f64 / instrs as f64 * 1000_f64)
            }
            Counter::StalledCyclesFrontend | Counter::StalledCyclesBackend => {
                let cycles = result.get(Counter::Cycles).unwrap().value;
                let stalled = value.value;
                format!("{:.2}%", stalled as f64 / cycles as f64 * 100_f64)
            }
            _ => "".to_string(),
        };
        table.add_row(vec![
            Cell::new(cntr.name()),
            Cell::new(value.value.to_formatted_string(&Locale::en))
                .set_alignment(CellAlignment::Right),
            Cell::new(value_str),
            Cell::new(format!("{:.2}", value.scaling)).set_alignment(CellAlignment::Right),
            Cell::new(cntr.description()),
        ]);
    }

    println!(
        "

Perf counters:

{table}"
    );

    Ok(())
}
