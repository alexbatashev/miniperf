use mperf_data::Scenario;
use pmu::Counter;
use pmu_data::{arith_parser::Expr, TmaScenario};
use std::collections::BTreeSet;

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
                "stalled_cycles_frontend" => Counter::StalledCyclesFrontend,
                "stalled_cycles_backend" => Counter::StalledCyclesBackend,
                _ => Counter::Custom(evt.clone()),
            })
            .chain([
                Counter::CpuClock,
                Counter::CpuMigrations,
                Counter::PageFaults,
                Counter::ContextSwitches,
            ])
            .collect(),
    }
}

/// Resolves and validates the independent coherent groups used by TMA.
///
/// Groups are deliberately not merged: perf multiplexes groups independently,
/// and combining their values would make a ratio look precise when it is not.
pub fn get_tma_counter_groups(scenario: &TmaScenario) -> anyhow::Result<Vec<Vec<Counter>>> {
    let groups = if scenario.groups.is_empty() {
        anyhow::bail!("TMA scenario has no coherent counter groups")
    } else {
        &scenario.groups
    };
    let available = scenario.events.iter().collect::<BTreeSet<_>>();
    let capacity = pmu::host_max_counters();
    let mut resolved = Vec::with_capacity(groups.len());
    for group in groups {
        if group.events.is_empty() {
            anyhow::bail!("TMA group '{}' is empty", group.name);
        }
        if let Some(limit) = capacity {
            if group.events.len() > limit {
                anyhow::bail!(
                    "TMA group '{}' needs {} counters but this PMU has only {limit}; split the methodology into independent coherent formulas",
                    group.name, group.events.len()
                );
            }
        }
        for event in &group.events {
            if !available.contains(event) {
                anyhow::bail!(
                    "TMA group '{}' references undeclared event '{event}'",
                    group.name
                );
            }
        }
        resolved.push(
            group
                .events
                .iter()
                .map(|event| match event.as_str() {
                    "cycles" => Counter::Cycles,
                    "instructions" => Counter::Instructions,
                    "stalled_cycles_frontend" => Counter::StalledCyclesFrontend,
                    "stalled_cycles_backend" => Counter::StalledCyclesBackend,
                    _ => Counter::Custom(event.clone()),
                })
                .collect(),
        );
    }
    for metric in &scenario.metrics {
        let group = metric.group.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "TMA metric '{}' does not name a coherent group",
                metric.name
            )
        })?;
        let group = groups
            .iter()
            .find(|candidate| candidate.name == *group)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TMA metric '{}' references unknown group '{group}'",
                    metric.name
                )
            })?;
        let mut variables = BTreeSet::new();
        let expression = pmu_data::arith_parser::try_parse_expr(&metric.formula)
            .map_err(|error| anyhow::anyhow!("invalid TMA formula '{}': {error}", metric.name))?;
        formula_variables(&expression, &mut variables);
        for variable in variables {
            if scenario
                .metrics
                .iter()
                .any(|candidate| candidate.name == variable)
            {
                continue;
            }
            if !group.events.contains(&variable) {
                anyhow::bail!(
                    "TMA metric '{}' uses '{variable}' outside coherent group '{}'",
                    metric.name,
                    group.name
                );
            }
        }
    }
    Ok(resolved)
}

fn formula_variables(expression: &Expr, variables: &mut BTreeSet<String>) {
    match expression {
        Expr::Variable(name) => {
            variables.insert(name.clone());
        }
        Expr::Binary { lhs, rhs, .. } => {
            formula_variables(lhs, variables);
            formula_variables(rhs, variables);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                formula_variables(arg, variables);
            }
        }
        Expr::Constant(_) | Expr::Num(_) => {}
    }
}
