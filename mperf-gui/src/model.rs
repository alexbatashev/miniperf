use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use mperf_data::{scenario_ui, RecordInfo};
use num_format::{Locale, ToFormattedString};
use pmu_data::TabSpec;
use sqlite::Connection;

use crate::{flamegraph::FlamegraphData, metrics::MetricsTableData};

#[derive(Debug)]
pub struct ResultsModel {
    pub result_directory: PathBuf,
    pub record_info: RecordInfo,
    pub summary: SummaryStats,
    pub tabs: Vec<GuiTab>,
}

#[derive(Debug)]
pub enum GuiTab {
    Summary,
    MetricsTable {
        title: String,
        data: MetricsTableData,
    },
    Loops,
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

        let database_path = result_directory.join("perf.db");
        fs::metadata(&database_path)
            .with_context(|| format!("failed to access {}", database_path.display()))?;
        let connection = sqlite::open(&database_path)
            .with_context(|| format!("failed to open {}", database_path.display()))?;
        let summary = SummaryStats::load(&connection)?;

        let tabs = scenario_ui(&record_info)
            .tabs
            .into_iter()
            .map(|tab| GuiTab::load(tab, &connection, &result_directory))
            .collect();

        Ok(Self {
            result_directory,
            record_info,
            summary,
            tabs,
        })
    }
}

impl GuiTab {
    pub fn title(&self) -> &str {
        match self {
            Self::Summary => "Summary",
            Self::MetricsTable { title, .. } => title,
            Self::Loops => "Loops",
            Self::Flamegraph(_) => "Flamegraph",
        }
    }

    fn load(tab: TabSpec, connection: &Connection, result_directory: &Path) -> Self {
        match tab {
            TabSpec::Summary => Self::Summary,
            TabSpec::MetricsTable(spec) => {
                let title = spec.title.clone().unwrap_or_else(|| spec.view.clone());
                Self::MetricsTable {
                    title,
                    data: MetricsTableData::load(connection, &spec),
                }
            }
            TabSpec::Loops => Self::Loops,
            TabSpec::Flamegraph => {
                Self::Flamegraph(Box::new(FlamegraphData::load(result_directory)))
            }
        }
    }
}

impl SummaryStats {
    fn load(connection: &Connection) -> Result<Self> {
        let available_columns: HashSet<String> = connection
            .prepare("PRAGMA table_info(pmu_counters);")
            .context("failed to inspect pmu_counters")?
            .into_iter()
            .map(|row| {
                let row = row.context("failed to read pmu_counters schema")?;
                Ok(row.read::<&str, _>("name").to_string())
            })
            .collect::<Result<_>>()?;

        let has_branch = available_columns.contains("pmu_branch_instructions")
            && available_columns.contains("pmu_branch_misses");
        let has_cache = available_columns.contains("pmu_llc_references")
            && available_columns.contains("pmu_llc_misses");
        let has_stalled = available_columns.contains("pmu_stalled_cycles_frontend")
            && available_columns.contains("pmu_stalled_cycles_backend");

        let mut select_parts = vec![
            "SUM(pmu_cycles) AS pmu_cycles".to_string(),
            "SUM(pmu_instructions) AS pmu_instructions".to_string(),
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
        let mut rows = connection
            .prepare(&query)
            .context("failed to prepare summary query")?
            .into_iter();
        let row = rows
            .next()
            .context("summary query returned no rows")?
            .context("failed to read summary row")?;

        let read = |name| -> Result<u64> {
            Ok(row
                .try_read::<Option<i64>, _>(name)
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
            "CAST(SUM({column} * 1.0 / confidence) AS INTEGER) AS {column}"
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

    #[test]
    fn loads_required_and_optional_summary_counters() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE pmu_counters (
                    confidence REAL,
                    pmu_cycles INTEGER,
                    pmu_instructions INTEGER,
                    pmu_branch_instructions INTEGER,
                    pmu_branch_misses INTEGER
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
}
