use mperf_data::{RecordInfo, Scenario, ScenarioInfo};
use pmu_data::{MetricColumnSpec, MetricsTableSpec, OrderSpec, ScenarioUi, SortDirection, TabSpec};

pub fn scenario_ui(record: &RecordInfo) -> ScenarioUi {
    match record.scenario {
        Scenario::Snapshot => snapshot_ui(),
        Scenario::Roofline => roofline_ui(),
        Scenario::TMA => match &record.scenario_info {
            ScenarioInfo::TMA(tma) => tma.ui.clone().unwrap_or_else(|| tma_fallback_ui(tma)),
            _ => snapshot_ui(),
        },
    }
}

fn snapshot_ui() -> ScenarioUi {
    ScenarioUi {
        tabs: vec![
            TabSpec::Summary,
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
        ],
    }
}

fn roofline_ui() -> ScenarioUi {
    ScenarioUi {
        tabs: vec![TabSpec::Summary, TabSpec::Loops, TabSpec::Flamegraph],
    }
}

fn tma_fallback_ui(tma: &mperf_data::TMAInfo) -> ScenarioUi {
    let mut ui = snapshot_ui();
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
        let ui = snapshot_ui();
        assert_eq!(ui.tabs.len(), 3);
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
