use anyhow::Result;
use comfy_table::{Cell, CellAlignment, Color, Table};
use num_format::{Locale, ToFormattedString};
use pmu::{Counter, Process};

pub fn do_stat(command: Vec<String>) -> Result<()> {
    let process = Process::new(&command, &[])?;

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
            Counter::CpuClock,
            Counter::CpuMigrations,
            Counter::PageFaults,
            Counter::ContextSwitches,
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
        let info = match cntr {
            Counter::Instructions | Counter::BranchInstructions => {
                let cycles = result.get(Counter::Cycles).unwrap().value;
                let instrs = value.value;
                let ipc = instrs as f64 / cycles as f64;
                let mut cell = Cell::new(format!("{:.2} inst/cycle", ipc));

                if cntr == Counter::Instructions {
                    if ipc < 0.6 {
                        cell = cell.fg(Color::Red);
                    } else if ipc < 1.5 {
                        cell = cell.fg(Color::Yellow);
                    }
                }

                cell
            }
            Counter::BranchMisses | Counter::LLCMisses => {
                let instrs = result.get(Counter::Instructions).unwrap().value;
                let misses = value.value;
                Cell::new(format!(
                    "{:.2} MPKI",
                    misses as f64 / instrs as f64 * 1000_f64
                ))
            }
            Counter::StalledCyclesFrontend | Counter::StalledCyclesBackend => {
                let cycles = result.get(Counter::Cycles).unwrap().value;
                let stalled = value.value;
                let percentage = stalled as f64 / cycles as f64 * 100_f64;
                let mut cell = Cell::new(format!("{:.2}%", percentage));

                if percentage > 20_f64 {
                    cell = cell.fg(Color::Red);
                } else if percentage > 10_f64 {
                    cell = cell.fg(Color::Yellow);
                }

                cell
            }
            _ => Cell::new(""),
        };
        table.add_row(vec![
            Cell::new(cntr.name()),
            Cell::new(value.value.to_formatted_string(&Locale::en))
                .set_alignment(CellAlignment::Right),
            info,
            Cell::new(format!("{:.2}", value.scaling)).set_alignment(CellAlignment::Right),
            Cell::new(cntr.description()),
        ]);
    }

    println!(
        "

Performance counter stats for '{}':

{table}",
        command.join(" ")
    );

    Ok(())
}
