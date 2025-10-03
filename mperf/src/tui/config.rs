use mperf_data::{RecordInfo, Scenario, ScenarioInfo};
use pmu_data::{MetricColumnSpec, MetricsTableSpec, OrderSpec, ScenarioUi, SortDirection, TabSpec};

pub fn scenario_ui(record: &RecordInfo) -> ScenarioUi {
    match record.scenario {
        Scenario::Snapshot => snapshot_ui(),
        Scenario::Roofline => roofline_ui(),
        Scenario::TMA => match &record.scenario_info {
            ScenarioInfo::TMA(tma) => tma.ui.clone().unwrap_or_else(|| tma_fallback_ui()),
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

fn tma_fallback_ui() -> ScenarioUi {
    // Default to snapshot layout if platform JSON does not provide UI description
    snapshot_ui()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_ui_contains_expected_tabs() {
        let ui = snapshot_ui();
        assert_eq!(ui.tabs.len(), 3);
    }
}
