use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use mperf_data::{RecordInfo, ScenarioInfo, scenario_ui};
use num_format::{Locale, ToFormattedString};
use pmu_data::{TabSpec, TmaMetric};
use store::Session;

use crate::{
    flamegraph::FlamegraphData,
    memory::MemoryData,
    metrics::MetricsTableData,
    profile::ProfileData,
    roofline::RooflineData,
    snapshot::SnapshotData,
    sql::{Connection, SqlResult, table_columns},
};

#[derive(Debug)]
pub struct ResultsModel {
    pub result_directory: PathBuf,
    pub record_info: RecordInfo,
    pub summary: SummaryStats,
    pub tma_summary: Option<TmaSummaryData>,
    pub profile: ProfileData,
    pub snapshot: Option<SnapshotData>,
    pub tabs: Vec<GuiTab>,
    /// Viewer-attached visualization manifest (`manifest.yaml`/`.json` in the
    /// session directory); presentation-only, absence never hides data.
    /// Consumed by the redesigned track-based views.
    #[allow(dead_code)]
    pub manifest: Option<mperf_data::VisualizationManifest>,
}

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

#[derive(Debug)]
pub enum GuiTab {
    Summary,
    MetricsTable {
        title: String,
        data: MetricsTableData,
    },
    Loops(Box<RooflineData>),
    Memory(Box<MemoryData>),
    Flamegraph(Box<FlamegraphData>),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryStats {
    pub cycles: u64,
    pub instructions: u64,
    pub branch_instructions: Option<u64>,
    pub branch_misses: Option<u64>,
    pub cache_references: Option<u64>,
    pub cache_misses: Option<u64>,
    pub stalled_cycles_frontend: Option<u64>,
    pub stalled_cycles_backend: Option<u64>,
}

#[derive(Debug)]
pub struct CounterRow {
    pub label: &'static str,
    pub value: String,
    pub detail: String,
}

impl ResultsModel {
    pub fn load(result_directory: impl AsRef<Path>) -> Result<Self> {
        let result_directory = fs::canonicalize(result_directory.as_ref()).with_context(|| {
            format!("failed to resolve {}", result_directory.as_ref().display())
        })?;
        let info_path = result_directory.join("info.json");
        let info_data = fs::read_to_string(&info_path)
            .with_context(|| format!("failed to read {}", info_path.display()))?;
        let record_info: RecordInfo = serde_json::from_str(&info_data)
            .with_context(|| format!("failed to parse {}", info_path.display()))?;
        record_info.ensure_supported_format()?;

        let session = Session::open(&result_directory)
            .with_context(|| format!("failed to open {}", result_directory.display()))?;
        let connection = session.connection();
        let summary = SummaryStats::load(connection)?;
        let tma_summary = TmaSummaryData::for_scenario(&record_info.scenario_info, connection);
        let mut profile = ProfileData::load(connection);
        profile.logical_cpu_count = record_info.logical_cpu_count;
        let snapshot = SnapshotData::load(connection, record_info.logical_cpu_count);

        let tabs = scenario_ui(&record_info)
            .tabs
            .into_iter()
            .map(|tab| GuiTab::load(tab, connection, &result_directory, &record_info))
            .collect();

        let manifest = ["manifest.yaml", "manifest.yml", "manifest.json"]
            .iter()
            .map(|name| result_directory.join(name))
            .find(|path| path.exists())
            .and_then(
                |path| match mperf_data::VisualizationManifest::load(&path) {
                    Ok(manifest) => Some(manifest),
                    Err(error) => {
                        eprintln!("ignoring visualization manifest: {error}");
                        None
                    }
                },
            );

        Ok(Self {
            result_directory,
            record_info,
            summary,
            tma_summary,
            profile,
            snapshot,
            tabs,
            manifest,
        })
    }
}

impl TmaSummaryData {
    fn for_scenario(scenario: &ScenarioInfo, connection: &Connection) -> Option<Self> {
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

impl GuiTab {
    fn load(
        tab: TabSpec,
        connection: &Connection,
        result_directory: &Path,
        record_info: &RecordInfo,
    ) -> Self {
        match tab {
            TabSpec::Summary => Self::Summary,
            TabSpec::Resources => {
                let spec = mperf_data::resources_table_spec();
                Self::MetricsTable {
                    title: spec.title.clone().unwrap_or_else(|| spec.view.clone()),
                    data: MetricsTableData::load(connection, &spec),
                }
            }
            TabSpec::MetricsTable(spec) => {
                let title = spec.title.clone().unwrap_or_else(|| spec.view.clone());
                Self::MetricsTable {
                    title,
                    data: MetricsTableData::load(connection, &spec),
                }
            }
            TabSpec::Loops => Self::Loops(Box::new(RooflineData::load(
                connection,
                record_info
                    .cpu_info
                    .roofline_calibration
                    .as_deref()
                    .cloned(),
                match &record_info.scenario_info {
                    ScenarioInfo::Roofline(info) => info.method.as_deref().cloned(),
                    _ => None,
                },
            ))),
            TabSpec::Memory => Self::Memory(Box::new(MemoryData::load(
                connection,
                record_info
                    .cpu_info
                    .memory_calibration
                    .as_deref()
                    .map(|calibration| calibration.memory_levels.clone())
                    .unwrap_or_default(),
            ))),
            TabSpec::Flamegraph => {
                Self::Flamegraph(Box::new(FlamegraphData::load(result_directory)))
            }
        }
    }
}

impl SummaryStats {
    fn load(connection: &Connection) -> Result<Self> {
        let available_columns: HashSet<String> = table_columns(connection, "pmu_counters")
            .into_iter()
            .map(|column| column.name)
            .collect();

        let has_branch = available_columns.contains("pmu_branch_instructions")
            && available_columns.contains("pmu_branch_misses");
        let has_cache = available_columns.contains("pmu_llc_references")
            && available_columns.contains("pmu_llc_misses");
        let has_stalled = available_columns.contains("pmu_stalled_cycles_frontend")
            && available_columns.contains("pmu_stalled_cycles_backend");

        let mut select_parts = vec![
            "CAST(SUM(pmu_cycles) AS BIGINT) AS pmu_cycles".to_string(),
            "CAST(SUM(pmu_instructions) AS BIGINT) AS pmu_instructions".to_string(),
        ];

        push_optional_sum(&mut select_parts, has_branch, "pmu_branch_instructions");
        push_optional_sum(&mut select_parts, has_branch, "pmu_branch_misses");
        push_optional_sum(&mut select_parts, has_cache, "pmu_llc_references");
        push_optional_sum(&mut select_parts, has_cache, "pmu_llc_misses");
        push_optional_sum(
            &mut select_parts,
            has_stalled,
            "pmu_stalled_cycles_frontend",
        );
        push_optional_sum(&mut select_parts, has_stalled, "pmu_stalled_cycles_backend");

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
            cache_references: has_cache.then(|| read("pmu_llc_references")).transpose()?,
            cache_misses: has_cache.then(|| read("pmu_llc_misses")).transpose()?,
            stalled_cycles_frontend: has_stalled
                .then(|| read("pmu_stalled_cycles_frontend"))
                .transpose()?,
            stalled_cycles_backend: has_stalled
                .then(|| read("pmu_stalled_cycles_backend"))
                .transpose()?,
        })
    }

    pub fn rows(self) -> Vec<CounterRow> {
        let ipc = ratio(self.instructions, self.cycles, 1.0, "");
        let branch_per_cycle = optional_ratio(
            self.branch_instructions,
            Some(self.cycles),
            1.0,
            " per cycle",
        );
        let branch_miss_percent =
            optional_ratio(self.branch_misses, self.branch_instructions, 100.0, "%");
        let branch_mpki = optional_ratio(self.branch_misses, Some(self.instructions), 1000.0, "");
        let cache_miss_percent = match (self.cache_misses, self.cache_references) {
            (Some(misses), Some(references)) if misses + references > 0 => {
                format!(
                    "{:.2}%",
                    misses as f64 / (misses + references) as f64 * 100.0
                )
            }
            _ => "N/A".to_string(),
        };
        let cache_mpki = optional_ratio(self.cache_misses, Some(self.instructions), 1000.0, "");

        vec![
            row("Cycles", count(self.cycles), ""),
            row("Instructions", count(self.instructions), ""),
            row("IPC", ipc, ""),
            row(
                "Branch instructions",
                optional_count(self.branch_instructions),
                branch_per_cycle,
            ),
            row(
                "Branch misses",
                optional_count(self.branch_misses),
                branch_miss_percent,
            ),
            row("Branch MPKI", branch_mpki, ""),
            row(
                "Last level cache references",
                optional_count(self.cache_references),
                "",
            ),
            row(
                "Last level cache misses",
                optional_count(self.cache_misses),
                cache_miss_percent,
            ),
            row("Cache MPKI", cache_mpki, ""),
            row(
                "Stalled cycles backend",
                optional_count(self.stalled_cycles_backend),
                optional_ratio(self.stalled_cycles_backend, Some(self.cycles), 100.0, "%"),
            ),
            row(
                "Stalled cycles frontend",
                optional_count(self.stalled_cycles_frontend),
                optional_ratio(self.stalled_cycles_frontend, Some(self.cycles), 100.0, "%"),
            ),
        ]
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

fn row(label: &'static str, value: impl Into<String>, detail: impl Into<String>) -> CounterRow {
    CounterRow {
        label,
        value: value.into(),
        detail: detail.into(),
    }
}

fn count(value: u64) -> String {
    value.to_formatted_string(&Locale::en)
}

fn optional_count(value: Option<u64>) -> String {
    value.map(count).unwrap_or_else(|| "N/A".to_string())
}

fn ratio(numerator: u64, denominator: u64, scale: f64, suffix: &str) -> String {
    if denominator == 0 {
        "N/A".to_string()
    } else {
        format!(
            "{:.2}{suffix}",
            numerator as f64 / denominator as f64 * scale
        )
    }
}

fn optional_ratio(
    numerator: Option<u64>,
    denominator: Option<u64>,
    scale: f64,
    suffix: &str,
) -> String {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator)) => ratio(numerator, denominator, scale, suffix),
        _ => "N/A".to_string(),
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
        assert_eq!(summary.cache_misses, None);
    }

    #[test]
    fn zero_denominators_render_as_na() {
        assert_eq!(ratio(10, 0, 1.0, ""), "N/A");
        assert_eq!(optional_ratio(Some(10), None, 100.0, "%"), "N/A");
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

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn bench_load_real_directory() {
        let Ok(dir) = std::env::var("MPERF_BENCH_DIR") else {
            return;
        };
        let t0 = std::time::Instant::now();
        let model = ResultsModel::load(&dir).unwrap();
        println!("ResultsModel::load: {:?}", t0.elapsed());
        println!(
            "samples={} frames={} cpu_obs={}",
            model.profile.samples.len(),
            model.profile.frames.len(),
            model.profile.cpu_observations.len()
        );

        let t = std::time::Instant::now();
        let tree = crate::profile_analysis::CallTree::build(
            &model.profile,
            &crate::profile_analysis::SampleFilter::default(),
        );
        println!(
            "CallTree::build: {:?} ({} nodes)",
            t.elapsed(),
            tree.nodes.len()
        );

        let t = std::time::Instant::now();
        let layout = tree.icicle_layout(None);
        println!(
            "icicle_layout: {:?} ({} frames)",
            t.elapsed(),
            layout.frames.len()
        );

        let t = std::time::Instant::now();
        let chart =
            crate::views::flame_canvas::FlameChart::from_icicle_layout(&layout, &tree, true);
        println!(
            "FlameChart::from_icicle_layout: {:?} ({} frames)",
            t.elapsed(),
            chart.frames.len()
        );

        let t = std::time::Instant::now();
        let analysis = crate::profile_analysis::FunctionAnalysis::build(
            &model.profile,
            &crate::profile_analysis::SampleFilter::default(),
        );
        println!(
            "FunctionAnalysis::build: {:?} ({} functions)",
            t.elapsed(),
            analysis.functions.len()
        );

        for tab in &model.tabs {
            if let GuiTab::Flamegraph(data) = tab {
                let t = std::time::Instant::now();
                let layout = data.layout(false, flamelens::flame::ROOT_ID).unwrap();
                println!(
                    "flame layout: {:?} ({} frames)",
                    t.elapsed(),
                    layout.frames.len()
                );
                let t = std::time::Instant::now();
                let chart = crate::views::flame_canvas::FlameChart::from_flame_layout(&layout);
                println!(
                    "FlameChart::from_flame_layout: {:?} ({} frames)",
                    t.elapsed(),
                    chart.frames.len()
                );
            }
        }
    }
}
