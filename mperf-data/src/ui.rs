use pmu_data::{MetricColumnSpec, MetricsTableSpec, OrderSpec, ScenarioUi, SortDirection, TabSpec};

use crate::{RecordInfo, Scenario, ScenarioInfo};

/// Returns the result-view tabs for a recorded scenario.
///
/// This is shared by the TUI and GUI so both frontends expose the same
/// scenario-specific views and use the UI definition embedded in TMA results.
pub fn scenario_ui(record: &RecordInfo) -> ScenarioUi {
    match record.scenario {
        Scenario::Snapshot => snapshot_ui(matches!(
            &record.scenario_info,
            ScenarioInfo::Snapshot(info) if info.scope != "legacy_root_only"
        )),
        Scenario::Mem => memory_ui(),
        Scenario::Roofline => roofline_ui(record),
        Scenario::TMA => match &record.scenario_info {
            ScenarioInfo::TMA(tma) => tma.ui.clone().unwrap_or_else(|| tma_fallback_ui(tma)),
            _ => snapshot_ui(false),
        },
    }
}

/// Table configuration used by compact frontends for USE findings.
pub fn resources_table_spec() -> MetricsTableSpec {
    let column = |key: &str, label: &str, width: u16| MetricColumnSpec {
        key: key.to_string(),
        label: Some(label.to_string()),
        format: pmu_data::ValueFormat::Text,
        width: Some(width),
        sticky: false,
        optional: false,
    };
    MetricsTableSpec {
        view: "snapshot_findings".to_string(),
        title: Some("Resources / What to measure next".to_string()),
        include_default_columns: false,
        columns: vec![
            column("severity", "Severity", 10),
            column("resource", "Resource", 12),
            column("finding", "Finding", 44),
            column("evidence", "Evidence", 46),
            column("recommendation", "Next measurement", 72),
            column("scope", "Scope", 22),
            column("quality", "Quality", 18),
        ],
        order_by: Some(OrderSpec {
            column: "rank".to_string(),
            direction: SortDirection::Asc,
        }),
        limit: Some(50),
        sticky_columns: Some(2),
        function_column: None,
        enable_assembly: false,
    }
}

fn memory_ui() -> ScenarioUi {
    ScenarioUi {
        tabs: vec![TabSpec::Summary, TabSpec::Memory, TabSpec::Flamegraph],
    }
}

fn snapshot_ui(expanded_resources: bool) -> ScenarioUi {
    let mut tabs = vec![TabSpec::Summary];
    if expanded_resources {
        tabs.push(TabSpec::Resources);
    }
    tabs.extend([
        TabSpec::MetricsTable(MetricsTableSpec {
            view: "hotspots".to_string(),
            title: Some("Hotspots".to_string()),
            include_default_columns: true,
            columns: vec![
                MetricColumnSpec {
                    key: "branch_miss_rate".to_string(),
                    label: Some("Branch miss rate".to_string()),
                    format: pmu_data::ValueFormat::Percent2,
                    width: Some(20),
                    sticky: false,
                    optional: false,
                },
                MetricColumnSpec {
                    key: "branch_mpki".to_string(),
                    label: Some("Branch MPKI".to_string()),
                    format: pmu_data::ValueFormat::Float2,
                    width: Some(15),
                    sticky: false,
                    optional: false,
                },
                MetricColumnSpec {
                    key: "cache_miss_rate".to_string(),
                    label: Some("Cache miss rate".to_string()),
                    format: pmu_data::ValueFormat::Percent2,
                    width: Some(20),
                    sticky: false,
                    optional: false,
                },
                MetricColumnSpec {
                    key: "cache_mpki".to_string(),
                    label: Some("Cache MPKI".to_string()),
                    format: pmu_data::ValueFormat::Float2,
                    width: Some(15),
                    sticky: false,
                    optional: false,
                },
            ],
            order_by: Some(OrderSpec {
                column: "total".to_string(),
                direction: SortDirection::Desc,
            }),
            limit: Some(50),
            sticky_columns: Some(1),
            function_column: Some("func_name".to_string()),
            enable_assembly: true,
        }),
        TabSpec::Flamegraph,
    ]);
    ScenarioUi { tabs }
}

fn roofline_ui(record: &RecordInfo) -> ScenarioUi {
    let has_memory = match &record.scenario_info {
        ScenarioInfo::Roofline(info) => {
            info.method
                .as_deref()
                .is_some_and(|method| method.accounting == "qemu")
                || info.backend.contains("qemu")
        }
        _ => false,
    };
    let mut tabs = vec![TabSpec::Summary, TabSpec::Loops];
    if has_memory {
        tabs.push(TabSpec::Memory);
    }
    tabs.push(TabSpec::Flamegraph);
    ScenarioUi { tabs }
}

fn tma_fallback_ui(tma: &crate::TMAInfo) -> ScenarioUi {
    let mut ui = snapshot_ui(false);
    for tab in &mut ui.tabs {
        if let TabSpec::MetricsTable(table) = tab {
            table.view = "tma".to_string();
            table.columns = tma
                .metrics
                .iter()
                .map(|metric| MetricColumnSpec {
                    key: metric.name.replace('.', "_"),
                    label: Some(metric.name.clone()),
                    format: pmu_data::ValueFormat::Percent2,
                    width: Some(24),
                    sticky: false,
                    optional: false,
                })
                .collect();
        }
    }
    ui
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_ui_contains_expected_tabs() {
        let legacy = snapshot_ui(false);
        assert_eq!(legacy.tabs.len(), 3);
        let expanded = snapshot_ui(true);
        assert_eq!(expanded.tabs.len(), 4);
    }

    #[test]
    fn tma_fallback_exposes_computed_metrics() {
        let record: RecordInfo = serde_json::from_str(
            r#"{"format_version":2,"scenario":"TMA","command":null,"cpu_model":"test","cpu_vendor":"test","cores":[],"scenario_info":{"TMA":{"pid":1,"counters":[],"groups":[],"precise_attribution":false,"metrics":[{"name":"be_bound.memory_bound","desc":"Memory bound","formula":"0","group":null}],"constants":[],"ui":null}}}"#,
        )
        .unwrap();

        let ui = scenario_ui(&record);
        let table = ui
            .tabs
            .iter()
            .find_map(|tab| match tab {
                TabSpec::MetricsTable(table) => Some(table),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.view, "tma");
        assert_eq!(table.columns[0].key, "be_bound_memory_bound");
    }
}
