use anyhow::Result;
use comfy_table::{Cell, CellAlignment, Color, Table};
use num_format::{Locale, ToFormattedString};
use pmu::{Counter, CounterValue, MeasurementQuality, Process};

/// PMU (hardware) counters, shown per-core on heterogeneous systems.
fn pmu_counters() -> Vec<Counter> {
    vec![
        Counter::Cycles,
        Counter::Instructions,
        Counter::LLCReferences,
        Counter::LLCMisses,
        Counter::BranchMisses,
        Counter::BranchInstructions,
        Counter::StalledCyclesBackend,
        Counter::StalledCyclesFrontend,
    ]
}

/// Software counters, which are not PMU/core specific.
fn software_counters() -> Vec<Counter> {
    vec![
        Counter::CpuClock,
        Counter::CpuMigrations,
        Counter::PageFaults,
        Counter::ContextSwitches,
    ]
}

pub fn do_stat(pid: Option<u32>, command: Vec<String>) -> Result<()> {
    if pid.is_none() && command.is_empty() {
        anyhow::bail!(
            "stat requires a command, or --pid with a command used as the measurement duration"
        );
    }

    let process = if pid.is_none() {
        Some(Process::new(&command, &[])?)
    } else if command.is_empty() {
        None
    } else {
        Some(Process::new(&command, &[])?)
    };

    let mut counters = pmu_counters();
    counters.extend(software_counters());

    let mut builder = pmu::CountingDriverBuilder::new()
        .counters(&counters)
        .process(process.as_ref());

    if let Some(pid) = pid {
        builder = builder.pid(Some(pid as i32));
    }

    let mut driver = builder.build()?;

    driver.reset()?;
    driver.start()?;
    if let Some(process) = &process {
        process.cont();
        process.wait()?;
    } else if let Some(pid) = pid {
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    driver.stop()?;

    let result = driver.counters()?;

    println!(
        "

Performance counter stats for '{}':
",
        if let Some(pid) = pid {
            format!("pid {pid}")
        } else {
            command.join(" ")
        }
    );

    let cores = result.cores();

    if cores.is_empty() {
        // Homogeneous system: a single table with everything, as before.
        let all = [pmu_counters(), software_counters()].concat();
        let table = render_table(&all, |c| result.get(c.clone()));
        println!("{table}");
    } else {
        // Heterogeneous system: one table per core cluster, then a faithful
        // total summed across all clusters.
        for core in &cores {
            let table = render_table(&pmu_counters(), |c| {
                result.get_for(&Some(core.clone()), c.clone())
            });
            println!("{} (cpus {})\n{table}\n", core.name, core.cpus);
        }

        let all = [pmu_counters(), software_counters()].concat();
        let table = render_table(&all, |c| result.get(c.clone()));
        println!("Total \u{2014} all cores (faithful sum)\n{table}");
    }

    Ok(())
}

/// Render one counter table for a given scope. `get` returns the counter value
/// within that scope (a single core, or the aggregate total).
fn render_table(counters: &[Counter], get: impl Fn(&Counter) -> Option<CounterValue>) -> Table {
    let cycles = get(&Counter::Cycles).map(|v| v.value);
    let instructions = get(&Counter::Instructions).map(|v| v.value);

    let mut table = Table::new();
    table.set_header(vec![
        "Counter",
        "Value",
        "Info",
        "Scaling",
        "Quality",
        "Description",
    ]);

    for cntr in counters {
        let Some(value) = get(cntr) else {
            continue;
        };

        let info = info_cell(cntr, &value, cycles, instructions);

        table.add_row(vec![
            Cell::new(cntr.name()),
            Cell::new(value.value.to_formatted_string(&Locale::en))
                .set_alignment(CellAlignment::Right),
            info,
            Cell::new(format!("{:.2}", value.scaling)).set_alignment(CellAlignment::Right),
            Cell::new(match value.quality {
                MeasurementQuality::Exact => "exact",
                MeasurementQuality::Scaled => "scaled",
                MeasurementQuality::Estimated => "estimated",
            }),
            Cell::new(cntr.description()),
        ]);
    }

    table
}

/// Compute the derived "Info" cell (IPC, MPKI, stall %) for a counter, relative
/// to the cycles/instructions of the same scope.
fn info_cell(
    cntr: &Counter,
    value: &CounterValue,
    cycles: Option<u64>,
    instructions: Option<u64>,
) -> Cell {
    match cntr {
        Counter::Instructions | Counter::BranchInstructions => {
            let Some(cycles) = cycles.filter(|c| *c > 0) else {
                return Cell::new("");
            };
            let ipc = value.value as f64 / cycles as f64;
            let mut cell = Cell::new(format!("{:.2} inst/cycle", ipc));

            if *cntr == Counter::Instructions {
                if ipc < 0.6 {
                    cell = cell.fg(Color::Red);
                } else if ipc < 1.5 {
                    cell = cell.fg(Color::Yellow);
                }
            }

            cell
        }
        Counter::BranchMisses | Counter::LLCMisses => {
            let Some(instructions) = instructions.filter(|i| *i > 0) else {
                return Cell::new("");
            };
            Cell::new(format!(
                "{:.2} MPKI",
                value.value as f64 / instructions as f64 * 1000_f64
            ))
        }
        Counter::StalledCyclesFrontend | Counter::StalledCyclesBackend => {
            let Some(cycles) = cycles.filter(|c| *c > 0) else {
                return Cell::new("");
            };
            let percentage = value.value as f64 / cycles as f64 * 100_f64;
            let mut cell = Cell::new(format!("{:.2}%", percentage));

            if percentage > 20_f64 {
                cell = cell.fg(Color::Red);
            } else if percentage > 10_f64 {
                cell = cell.fg(Color::Yellow);
            }

            cell
        }
        _ => Cell::new(""),
    }
}
