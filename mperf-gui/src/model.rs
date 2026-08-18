use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use mperf_data::ScenarioInfo;
use pmu_data::TmaMetric;

use crate::sql::{Connection, SqlResult, table_columns};

#[derive(Debug, Clone, PartialEq)]
pub struct TmaSummaryData {
    pub rows: Vec<TmaSummaryRow>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TmaSummaryRow {
    pub name: String,
    pub description: String,
    pub level: usize,
    pub value: Option<f64>,
    pub dominant: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryStats {
    pub cycles: u64,
    pub instructions: u64,
    pub branch_instructions: Option<u64>,
    pub branch_misses: Option<u64>,
}

impl TmaSummaryData {
    pub fn for_scenario(scenario: &ScenarioInfo, connection: &Connection) -> Option<Self> {
        let ScenarioInfo::TMA(info) = scenario else {
            return None;
        };
        Some(Self::load(connection, &info.metrics))
    }

    fn load(connection: &Connection, metrics: &[TmaMetric]) -> Self {
        match load_persisted_tma_summary(connection) {
            Ok(persisted) => Self {
                rows: join_tma_summary(metrics, &persisted),
                error: None,
            },
            Err(error) => Self {
                rows: join_tma_summary(metrics, &HashMap::new()),
                error: Some(format!("{error:#}")),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PersistedTmaSummary {
    value: Option<f64>,
    dominant: bool,
}

fn load_persisted_tma_summary(
    connection: &Connection,
) -> Result<HashMap<String, PersistedTmaSummary>> {
    let mut statement = connection
        .prepare("SELECT metric, value, verdict FROM tma_summary;")
        .context("TMA summary is unavailable: failed to query table `tma_summary`")?;
    let rows = statement
        .query_map([], |row| {
            let metric = row.get::<_, String>(0)?;
            let value = finite_tma_value(row.get::<_, Option<f64>>(1)?);
            let dominant = row
                .get::<_, Option<String>>(2)?
                .is_some_and(|verdict| verdict.trim().eq_ignore_ascii_case("dominant"));
            Ok((metric, PersistedTmaSummary { value, dominant }))
        })
        .context("failed to read table `tma_summary`")?;

    rows.collect::<SqlResult<HashMap<_, _>>>()
        .context("failed to read a row from table `tma_summary`")
}

fn join_tma_summary(
    metrics: &[TmaMetric],
    persisted: &HashMap<String, PersistedTmaSummary>,
) -> Vec<TmaSummaryRow> {
    metrics
        .iter()
        .filter_map(|metric| {
            let level = tma_hierarchy_level(&metric.name);
            (1..=3).contains(&level).then(|| {
                let summary = persisted.get(&metric.name).copied().unwrap_or_default();
                TmaSummaryRow {
                    name: metric.name.clone(),
                    description: metric.desc.clone(),
                    level,
                    value: summary.value,
                    dominant: summary.dominant,
                }
            })
        })
        .collect()
}

fn tma_hierarchy_level(name: &str) -> usize {
    name.bytes().filter(|byte| *byte == b'.').count() + 1
}

fn finite_tma_value(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

impl SummaryStats {
    pub fn load(connection: &Connection) -> Result<Self> {
        let available_columns: HashSet<String> = table_columns(connection, "pmu_counters")
            .into_iter()
            .map(|column| column.name)
            .collect();

        let has_branch = available_columns.contains("pmu_branch_instructions")
            && available_columns.contains("pmu_branch_misses");

        let mut select_parts = vec![
            "CAST(SUM(pmu_cycles) AS BIGINT) AS pmu_cycles".to_string(),
            "CAST(SUM(pmu_instructions) AS BIGINT) AS pmu_instructions".to_string(),
        ];

        push_optional_sum(&mut select_parts, has_branch, "pmu_branch_instructions");
        push_optional_sum(&mut select_parts, has_branch, "pmu_branch_misses");

        let query = format!("SELECT {} FROM pmu_counters;", select_parts.join(",\n"));
        let mut statement = connection
            .prepare(&query)
            .context("failed to prepare summary query")?;
        let mut rows = statement.query([]).context("failed to run summary query")?;
        let row = rows
            .next()
            .context("failed to read summary row")?
            .context("summary query returned no rows")?;

        let read = |name| -> Result<u64> {
            Ok(row
                .get::<_, Option<i64>>(name)
                .with_context(|| format!("failed to read {name}"))?
                .unwrap_or_default() as u64)
        };

        Ok(Self {
            cycles: read("pmu_cycles")?,
            instructions: read("pmu_instructions")?,
            branch_instructions: has_branch
                .then(|| read("pmu_branch_instructions"))
                .transpose()?,
            branch_misses: has_branch.then(|| read("pmu_branch_misses")).transpose()?,
        })
    }
}

fn push_optional_sum(parts: &mut Vec<String>, present: bool, column: &str) {
    if present {
        parts.push(format!(
            "CAST(SUM({column} * 1.0 / confidence) AS BIGINT) AS {column}"
        ));
    } else {
        parts.push(format!("0 AS {column}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mperf_data::{RooflineInfo, SnapshotInfo, TMAInfo};

    fn metric(name: &str, description: &str) -> TmaMetric {
        TmaMetric {
            name: name.to_string(),
            desc: description.to_string(),
            formula: "0".to_string(),
            group: None,
            cpus: None,
        }
    }

    fn tma_scenario(metrics: Vec<TmaMetric>) -> ScenarioInfo {
        ScenarioInfo::TMA(TMAInfo {
            pid: 1,
            counters: Vec::new(),
            groups: Vec::new(),
            precise_attribution: false,
            metrics,
            constants: Vec::new(),
            ui: None,
        })
    }

    #[test]
    fn loads_required_and_optional_summary_counters() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE pmu_counters (
                    confidence DOUBLE,
                    pmu_cycles BIGINT,
                    pmu_instructions BIGINT,
                    pmu_branch_instructions BIGINT,
                    pmu_branch_misses BIGINT
                );
                INSERT INTO pmu_counters VALUES (0.5, 100, 60, 10, 2);",
            )
            .unwrap();

        let summary = SummaryStats::load(&connection).unwrap();
        assert_eq!(summary.cycles, 100);
        assert_eq!(summary.instructions, 60);
        assert_eq!(summary.branch_instructions, Some(20));
        assert_eq!(summary.branch_misses, Some(4));
    }

    #[test]
    fn tma_summary_preserves_scenario_order_and_hierarchy() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tma_summary (
                    metric TEXT PRIMARY KEY,
                    value DOUBLE,
                    verdict TEXT
                );
                INSERT INTO tma_summary VALUES
                    ('be_bound.memory_bound', 0.75, 'dominant'),
                    ('retiring', 0.5, NULL),
                    ('be_bound', 0.25, NULL),
                    ('be_bound.memory_bound.dram', NULL, NULL),
                    ('unknown', 1.0, 'dominant');",
            )
            .unwrap();
        let scenario = tma_scenario(vec![
            metric("retiring", "Retiring slots"),
            metric("be_bound", "Backend bound slots"),
            metric("be_bound.memory_bound", "Memory-bound slots"),
            metric("be_bound.memory_bound.dram", "DRAM-bound memory accesses"),
            metric("be_bound.memory_bound.dram.local", "A fourth-level metric"),
            metric("fe_bound", "Frontend bound slots"),
        ]);

        let summary = TmaSummaryData::for_scenario(&scenario, &connection).unwrap();

        assert_eq!(summary.error, None);
        assert_eq!(
            summary
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "retiring",
                "be_bound",
                "be_bound.memory_bound",
                "be_bound.memory_bound.dram",
                "fe_bound",
            ]
        );
        assert_eq!(
            summary.rows.iter().map(|row| row.level).collect::<Vec<_>>(),
            vec![1, 1, 2, 3, 1]
        );
        assert_eq!(summary.rows[0].description, "Retiring slots");
        assert_eq!(summary.rows[0].value, Some(0.5));
        assert!(!summary.rows[0].dominant);
        assert_eq!(summary.rows[2].value, Some(0.75));
        assert!(summary.rows[2].dominant);
        assert_eq!(summary.rows[3].value, None);
        assert_eq!(summary.rows[4].value, None);
        assert!(!summary.rows[4].dominant);
        assert_eq!(finite_tma_value(Some(f64::INFINITY)), None);
        assert_eq!(finite_tma_value(Some(f64::NAN)), None);
    }

    #[test]
    fn tma_summary_keeps_legacy_errors_and_is_absent_for_other_scenarios() {
        let connection = Connection::open_in_memory().unwrap();
        let scenario = tma_scenario(vec![metric("retiring", "Retiring slots")]);

        let summary = TmaSummaryData::for_scenario(&scenario, &connection).unwrap();
        assert_eq!(summary.rows.len(), 1);
        assert_eq!(summary.rows[0].value, None);
        assert!(!summary.rows[0].dominant);
        assert!(
            summary
                .error
                .as_deref()
                .is_some_and(|error| error.contains("tma_summary"))
        );

        let snapshot = ScenarioInfo::Snapshot(SnapshotInfo {
            pid: 1,
            counters: Vec::new(),
            scope: "legacy_root_only".to_string(),
            interval_ms: 1_000,
            stop_reason: String::new(),
            collectors: Vec::new(),
            warnings: Vec::new(),
        });
        let roofline = ScenarioInfo::Roofline(RooflineInfo {
            backend: "compiler".to_string(),
            perf_pid: 1,
            counters: Vec::new(),
            inst_pid: 2,
            method: None,
        });
        assert!(TmaSummaryData::for_scenario(&snapshot, &connection).is_none());
        assert!(TmaSummaryData::for_scenario(&roofline, &connection).is_none());
    }
}

