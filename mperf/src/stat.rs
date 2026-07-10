use std::collections::HashMap;

use anyhow::{anyhow, Result};
use comfy_table::{Cell, CellAlignment, Color, Table};
use num_format::{Locale, ToFormattedString};
use pmu::{Counter, CounterValue, Metric, Process};

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

pub fn do_stat(
    pid: Option<u32>,
    command: Vec<String>,
    event_names: Vec<String>,
    topdown_level: Option<u8>,
) -> Result<()> {
    if pid.is_none() && command.is_empty() {
        anyhow::bail!(
            "stat requires a command, or --pid with a command used as the measurement duration"
        );
    }

    let process = if pid.is_none() || !command.is_empty() {
        Some(Process::new(&command, &[])?)
    } else {
        None
    };

    let capabilities = pmu::capabilities();
    if !capabilities.hardware_counters {
        eprintln!("notice: no hardware PMU detected (VM/container or permissions); hardware counters may be unavailable");
    }

    let supported = pmu::list_supported_counters(pmu::DriverKind::Default);
    let host_metrics = pmu::host_metrics();
    let (mut counters, metrics) = if topdown_level.is_some() {
        let scenario =
            pmu::host_tma_scenario().expect("architectural TMA fallback is always available");
        let counters = scenario
            .events
            .iter()
            .map(|event| match event.as_str() {
                "cycles" => Counter::Cycles,
                "instructions" => Counter::Instructions,
                "stalled_cycles_frontend" => Counter::StalledCyclesFrontend,
                "stalled_cycles_backend" => Counter::StalledCyclesBackend,
                _ => Counter::Custom(event.clone()),
            })
            .collect();
        (counters, Vec::new())
    } else if event_names.is_empty() {
        let mut defaults = pmu_counters();
        defaults.extend(software_counters());
        let applicable = applicable_metrics(&host_metrics, &defaults);
        (defaults, applicable)
    } else {
        requested_counters_and_metrics(&event_names, &supported, &host_metrics)?
    };

    let mut driver = loop {
        match pmu::CountingDriverBuilder::new()
            .counters(&counters)
            .process(process.as_ref())
            .pid(pid.map(|pid| pid as i32))
            .build()
        {
            Ok(driver) => break driver,
            Err(error) if error.is_event_unsupported() => {
                let unsupported = error.counter_name().unwrap_or_default();
                let Some(index) = counters
                    .iter()
                    .position(|counter| counter.name() == unsupported)
                else {
                    return Err(error.into());
                };
                eprintln!("notice: {unsupported} is not supported by this PMU; omitting it");
                counters.remove(index);
                if counters.is_empty() {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    };

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

    if let Some(level) = topdown_level {
        let scenario =
            pmu::host_tma_scenario().expect("architectural TMA fallback is always available");
        render_topdown(&scenario, level, &result);
        return Ok(());
    }

    let selected_pmu: Vec<Counter> = counters
        .iter()
        .filter(|counter| !counter.is_software())
        .cloned()
        .collect();
    let selected_software: Vec<Counter> = counters
        .iter()
        .filter(|counter| counter.is_software())
        .cloned()
        .collect();

    println!(
        "

Performance counter stats for '{}':
",
        pid.map_or_else(|| command.join(" "), |pid| format!("pid {pid}"))
    );

    let cores = result.cores();

    if cores.is_empty() {
        // Homogeneous system: a single table with everything, as before.
        let table = render_table(&counters, &metrics, |c| result.get(c.clone()));
        println!("{table}");
    } else {
        // Heterogeneous system: one table per core cluster, then a faithful
        // total summed across all clusters.
        for core in &cores {
            let core_metrics = applicable_metrics(&metrics, &selected_pmu);
            let table = render_table(&selected_pmu, &core_metrics, |c| {
                result.get_for(&Some(core.clone()), c.clone())
            });
            println!("{} (cpus {})\n{table}\n", core.name, core.cpus);
        }

        let all = [selected_pmu, selected_software].concat();
        let table = render_table(&all, &metrics, |c| result.get(c.clone()));
        println!("Total \u{2014} all cores (faithful sum)\n{table}");
    }

    Ok(())
}

fn render_topdown(scenario: &pmu_data::TmaScenario, level: u8, result: &pmu::CounterResult) {
    let values = scenario
        .events
        .iter()
        .filter_map(|name| {
            let counter = match name.as_str() {
                "cycles" => Counter::Cycles,
                "instructions" => Counter::Instructions,
                "stalled_cycles_frontend" => Counter::StalledCyclesFrontend,
                "stalled_cycles_backend" => Counter::StalledCyclesBackend,
                _ => Counter::Custom(name.clone()),
            };
            result
                .get(counter)
                .map(|value| (name.clone(), value.value as f64))
        })
        .collect::<HashMap<_, _>>();
    let constants = scenario
        .constants
        .iter()
        .map(|constant| (constant.name.clone(), constant.value as f64))
        .collect::<HashMap<_, _>>();
    let mut rows = scenario
        .metrics
        .iter()
        .filter_map(|metric| {
            metric_depth(&metric.name)
                .le(&level)
                .then(|| {
                    eval_tma(&metric.formula, &values, &constants)
                        .ok()
                        .map(|value| (metric, value))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(_, left), (_, right)| right.total_cmp(left));
    println!("Top-down analysis ({})", scenario.name);
    for (index, (metric, value)) in rows.iter().enumerate() {
        let marker = if index == 0 { "*" } else { " " };
        let indent = "  ".repeat(metric_depth(&metric.name).saturating_sub(1) as usize);
        println!(
            "{marker} {indent}{:<28} {:>6.2}%  {}",
            metric
                .name
                .rsplit('.')
                .next()
                .unwrap_or(&metric.name)
                .replace('_', " "),
            value * 100.0,
            metric.desc
        );
    }
    println!("* dominant path at requested level");
}

fn metric_depth(name: &str) -> u8 {
    name.matches('.').count() as u8 + 1
}

fn eval_tma(
    formula: &str,
    values: &HashMap<String, f64>,
    constants: &HashMap<String, f64>,
) -> Result<f64> {
    fn eval(
        expr: &pmu_data::arith_parser::Expr,
        values: &HashMap<String, f64>,
        constants: &HashMap<String, f64>,
    ) -> Result<f64> {
        use pmu_data::arith_parser::{BinOp, Expr};
        match expr {
            Expr::Variable(name) => values
                .get(name)
                .copied()
                .ok_or_else(|| anyhow!("missing event {name}")),
            Expr::Constant(name) => constants
                .get(name)
                .copied()
                .ok_or_else(|| anyhow!("missing constant {name}")),
            Expr::Num(value) => Ok(*value),
            Expr::Binary { op, lhs, rhs } => {
                let (left, right) = (eval(lhs, values, constants)?, eval(rhs, values, constants)?);
                Ok(match op {
                    BinOp::Add => left + right,
                    BinOp::Sub => left - right,
                    BinOp::Mul => left * right,
                    BinOp::Div => left / right,
                    BinOp::Eq => (left == right) as u8 as f64,
                    BinOp::Lt => (left < right) as u8 as f64,
                    BinOp::Le => (left <= right) as u8 as f64,
                    BinOp::Gt => (left > right) as u8 as f64,
                    BinOp::Ge => (left >= right) as u8 as f64,
                })
            }
            Expr::Call { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| eval(arg, values, constants))
                    .collect::<Result<Vec<_>>>()?;
                match name.to_ascii_lowercase().as_str() {
                    "min" if args.len() == 2 => Ok(args[0].min(args[1])),
                    "max" if args.len() == 2 => Ok(args[0].max(args[1])),
                    "abs" if args.len() == 1 => Ok(args[0].abs()),
                    "if" if args.len() == 3 => Ok(if args[0] != 0.0 { args[1] } else { args[2] }),
                    _ => Err(anyhow!("unsupported TMA function {name}")),
                }
            }
        }
    }
    eval(
        &pmu_data::arith_parser::try_parse_expr(formula).map_err(|error| anyhow!(error))?,
        values,
        constants,
    )
}

/// Render one counter table for a given scope. `get` returns the counter value
/// within that scope (a single core, or the aggregate total).
fn render_table(
    counters: &[Counter],
    metrics: &[Metric],
    get: impl Fn(&Counter) -> Option<CounterValue>,
) -> Table {
    let cycles = get(&Counter::Cycles).map(|v| v.value);
    let instructions = get(&Counter::Instructions).map(|v| v.value);

    let mut table = Table::new();
    table.set_header(vec!["Counter", "Value", "Info", "Scaling", "Description"]);

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
            Cell::new(cntr.description()),
        ]);
    }

    let values: HashMap<String, f64> = counters
        .iter()
        .filter_map(|counter| {
            get(counter).map(|value| (counter.name().to_owned(), value.value as f64))
        })
        .collect();
    for metric in metrics {
        let Ok(value) = metric.expression.evaluate(&values) else {
            continue;
        };
        let rendered = metric.unit.as_deref().map_or_else(
            || format!("{value:.3}"),
            |unit| format!("{value:.3} {unit}"),
        );
        table.add_row(vec![
            Cell::new(&metric.name),
            Cell::new(rendered).set_alignment(CellAlignment::Right),
            Cell::new("derived"),
            Cell::new("-"),
            Cell::new(&metric.desc),
        ]);
    }

    table
}

fn requested_counters_and_metrics(
    names: &[String],
    supported: &[Counter],
    host_metrics: &[Metric],
) -> Result<(Vec<Counter>, Vec<Metric>)> {
    let mut counters = Vec::new();
    let mut metrics = Vec::new();
    for name in names {
        if let Some(counter) = find_counter(supported, name) {
            push_counter(&mut counters, counter.clone());
            continue;
        }
        let Some(metric) = host_metrics
            .iter()
            .find(|metric| metric.name.eq_ignore_ascii_case(name))
        else {
            return Err(anyhow!(
                "unknown event or metric '{name}'; run `mperf list` to see supported names"
            ));
        };
        for event_name in metric.expression.event_names().map_err(|error| {
            anyhow!(
                "metric '{}' has an invalid expression: {error}",
                metric.name
            )
        })? {
            let counter = find_counter(supported, &event_name).ok_or_else(|| {
                anyhow!(
                    "metric '{}' requires unavailable event '{event_name}'",
                    metric.name
                )
            })?;
            push_counter(&mut counters, counter.clone());
        }
        metrics.push(metric.clone());
    }
    Ok((counters, metrics))
}

fn applicable_metrics(metrics: &[Metric], counters: &[Counter]) -> Vec<Metric> {
    metrics
        .iter()
        .filter(|metric| {
            metric.expression.event_names().is_ok_and(|names| {
                names
                    .iter()
                    .all(|name| find_counter(counters, name).is_some())
            })
        })
        .cloned()
        .collect()
}

fn find_counter<'a>(counters: &'a [Counter], name: &str) -> Option<&'a Counter> {
    counters
        .iter()
        .find(|counter| counter.name().eq_ignore_ascii_case(name))
}

fn push_counter(counters: &mut Vec<Counter>, counter: Counter) {
    if find_counter(counters, counter.name()).is_none() {
        counters.push(counter);
    }
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

#[cfg(test)]
mod metric_tests {
    use super::*;
    use pmu::MetricExpression;

    fn ipc() -> Metric {
        Metric {
            name: "IPC".to_owned(),
            desc: "Instructions per cycle".to_owned(),
            expression: MetricExpression("instructions / cycles".to_owned()),
            unit: Some("insn/cycle".to_owned()),
        }
    }

    #[test]
    fn requested_metric_expands_required_counters() {
        let supported = vec![Counter::Cycles, Counter::Instructions];
        let (counters, metrics) =
            requested_counters_and_metrics(&["IPC".to_owned()], &supported, &[ipc()]).unwrap();
        assert_eq!(counters, supported);
        assert_eq!(metrics, vec![ipc()]);
    }

    #[test]
    fn applicable_metric_requires_every_event() {
        assert!(applicable_metrics(&[ipc()], &[Counter::Cycles]).is_empty());
        assert_eq!(
            applicable_metrics(&[ipc()], &[Counter::Cycles, Counter::Instructions]),
            vec![ipc()]
        );
    }
}
