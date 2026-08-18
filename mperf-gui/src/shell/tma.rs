//! Top-Down datasets: the metric hierarchy, the per-second interval shares and
//! the per-function level-1 breakdown. All three are recording-wide and load
//! once, with the session's sqlite connection.

use std::collections::{BTreeMap, HashMap};

use mperf_data::ScenarioInfo;
use sqlite::Connection;

use crate::model::{TmaSummaryData, TmaSummaryRow};

/// One second of pipeline-slot shares, aligned with [`TmaData::level1`].
#[derive(Clone, Debug, PartialEq)]
pub struct TmaInterval {
    pub start_ns: u64,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct TmaData {
    /// Hierarchy rows (levels 1..3) in the order the vendor spec declares them.
    pub rows: Vec<TmaSummaryRow>,
    /// Indices into `rows` of the level-1 metrics.
    pub level1: Vec<usize>,
    pub intervals: Vec<TmaInterval>,
    /// Level-1 shares per function name, aligned with `level1`.
    pub functions: HashMap<String, Vec<f64>>,
    pub error: Option<String>,
}

impl TmaData {
    /// Returns `None` for recordings whose scenario carries no TMA metrics.
    pub fn load(scenario: &ScenarioInfo, connection: &Connection) -> Option<Self> {
        let summary = TmaSummaryData::for_scenario(scenario, connection)?;
        let level1: Vec<usize> = summary
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.level == 1)
            .map(|(index, _)| index)
            .collect();
        let names: Vec<&str> = level1
            .iter()
            .map(|index| summary.rows[*index].name.as_str())
            .collect();

        Some(Self {
            intervals: load_intervals(connection, &names),
            functions: load_functions(connection, &names),
            level1,
            rows: summary.rows,
            error: summary.error,
        })
    }

    pub fn has_hierarchy(&self) -> bool {
        self.rows.iter().any(|row| row.value.is_some())
    }

    pub fn level1_rows(&self) -> impl Iterator<Item = &TmaSummaryRow> {
        self.level1.iter().map(|index| &self.rows[*index])
    }

    /// The chain of dominant metrics: the largest level-1 metric, then the
    /// largest of its children, and so on. Vendors expose different depths, so
    /// this walks the dotted names rather than a fixed tree.
    pub fn dominant_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut prefix: Option<String> = None;
        loop {
            let next = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| match prefix.as_deref() {
                    None => row.level == 1,
                    Some(parent) => is_child_of(&row.name, parent),
                })
                .filter_map(|(index, row)| Some((index, row.value?)))
                .max_by(|left, right| left.1.total_cmp(&right.1));
            match next {
                Some((index, _)) => {
                    path.push(index);
                    prefix = Some(self.rows[index].name.clone());
                }
                None => return path,
            }
        }
    }
}

pub fn is_child_of(name: &str, parent: &str) -> bool {
    name.strip_prefix(parent)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| !rest.contains('.'))
}

fn load_intervals(connection: &Connection, names: &[&str]) -> Vec<TmaInterval> {
    let Ok(statement) =
        connection.prepare("SELECT start_ns, metric, value FROM tma_intervals ORDER BY start_ns;")
    else {
        return Vec::new();
    };
    let mut by_start = BTreeMap::<u64, Vec<f64>>::new();
    for row in statement.into_iter() {
        let Ok(row) = row else { continue };
        let metric = row.read::<&str, _>(1);
        let Some(column) = names.iter().position(|name| *name == metric) else {
            continue;
        };
        let Ok(value) = row.try_read::<f64, _>(2) else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let start_ns = row.read::<i64, _>(0).max(0) as u64;
        by_start
            .entry(start_ns)
            .or_insert_with(|| vec![0.0; names.len()])[column] = value.clamp(0.0, 1.0);
    }
    by_start
        .into_iter()
        .map(|(start_ns, values)| TmaInterval { start_ns, values })
        .collect()
}

/// Level-1 shares per function from `VIEW tma`, whose columns spell the metric
/// names with underscores.
fn load_functions(connection: &Connection, names: &[&str]) -> HashMap<String, Vec<f64>> {
    let Ok(statement) = connection.prepare("SELECT * FROM tma;") else {
        return HashMap::new();
    };
    let columns: Vec<String> = names
        .iter()
        .map(|name| name.replace('.', "_"))
        .collect::<Vec<_>>();
    let mut functions = HashMap::new();
    for row in statement.into_iter() {
        let Ok(row) = row else { continue };
        let Ok(function) = row.try_read::<&str, _>("func_name") else {
            continue;
        };
        let values: Vec<f64> = columns
            .iter()
            .map(|column| {
                row.try_read::<f64, _>(column.as_str())
                    .ok()
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0)
            })
            .collect();
        if values.iter().sum::<f64>() > 0.0 {
            functions.insert(function.to_owned(), values);
        }
    }
    functions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<TmaSummaryRow> {
        [
            ("retiring", 0.3),
            ("backend_bound", 0.5),
            ("backend_bound.memory_bound", 0.4),
            ("backend_bound.memory_bound.dram_bound", 0.3),
            ("backend_bound.core_bound", 0.1),
        ]
        .into_iter()
        .map(|(name, value)| TmaSummaryRow {
            level: name.matches('.').count() + 1,
            name: name.to_owned(),
            description: String::new(),
            value: Some(value),
            dominant: false,
        })
        .collect()
    }

    fn data() -> TmaData {
        let rows = rows();
        TmaData {
            level1: (0..rows.len()).filter(|ix| rows[*ix].level == 1).collect(),
            rows,
            intervals: Vec::new(),
            functions: HashMap::new(),
            error: None,
        }
    }

    #[test]
    fn dominant_path_follows_the_largest_child_at_every_level() {
        let data = data();
        assert_eq!(
            data.dominant_path()
                .into_iter()
                .map(|index| data.rows[index].name.as_str())
                .collect::<Vec<_>>(),
            [
                "backend_bound",
                "backend_bound.memory_bound",
                "backend_bound.memory_bound.dram_bound"
            ]
        );
    }

    #[test]
    fn child_matching_only_accepts_the_immediate_level() {
        assert!(is_child_of("a.b", "a"));
        assert!(!is_child_of("a.b.c", "a"));
        assert!(!is_child_of("ab.c", "a"));
    }

    #[test]
    fn intervals_and_functions_come_from_the_recording_tables() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE tma_intervals (start_ns INTEGER, metric TEXT, value REAL);
                 INSERT INTO tma_intervals VALUES
                    (0, 'retiring', 0.4), (0, 'backend_bound', 0.6),
                    (1000000000, 'retiring', 0.2), (1000000000, 'unknown', 0.9);
                 CREATE VIEW tma AS SELECT 'hot' AS func_name, 0.3 AS retiring,
                    0.7 AS backend_bound;",
            )
            .unwrap();

        let names = ["retiring", "backend_bound"];
        let intervals = load_intervals(&connection, &names);
        assert_eq!(
            intervals,
            vec![
                TmaInterval {
                    start_ns: 0,
                    values: vec![0.4, 0.6]
                },
                TmaInterval {
                    start_ns: 1_000_000_000,
                    values: vec![0.2, 0.0]
                },
            ]
        );
        assert_eq!(
            load_functions(&connection, &names).get("hot"),
            Some(&vec![0.3, 0.7])
        );
    }
}
