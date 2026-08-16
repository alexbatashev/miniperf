use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{Context, Result};
use num_format::{Locale, ToFormattedString};
use pmu_data::{MetricColumnSpec, MetricsTableSpec, SortDirection, ValueFormat};

use crate::source::SourceLocation;
use crate::sql::{Connection, Value, as_f64, as_i64, as_text, table_columns};

#[derive(Debug)]
pub struct MetricsTableData {
    pub columns: Vec<MetricsColumn>,
    pub rows: Vec<MetricsRow>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct MetricsColumn {
    pub label: String,
    pub width: f32,
    pub align_right: bool,
}

#[derive(Debug)]
pub struct MetricsRow {
    pub values: Vec<String>,
    pub function: Option<String>,
    pub source: Option<SourceLocation>,
}

#[derive(Clone)]
struct ResolvedColumn {
    key: String,
    label: String,
    format: ValueFormat,
    width: Option<u16>,
}

impl MetricsTableData {
    pub fn load(connection: &Connection, spec: &MetricsTableSpec) -> Self {
        match load(connection, spec) {
            Ok(data) => data,
            Err(error) => Self {
                columns: display_columns(spec),
                rows: Vec::new(),
                error: Some(format!("{error:#}")),
            },
        }
    }

    pub fn total_width(&self) -> f32 {
        self.columns.iter().map(|column| column.width).sum()
    }
}

fn load(connection: &Connection, spec: &MetricsTableSpec) -> Result<MetricsTableData> {
    let available = available_columns(connection, &spec.view)?;
    let columns = resolved_columns(spec, &available)?;
    let query = build_query(spec, &available);
    let function_column = spec
        .function_column
        .as_deref()
        .or_else(|| available.contains("func_name").then_some("func_name"));
    let mut statement = connection
        .prepare(&query)
        .with_context(|| format!("failed to query view `{}`", spec.view))?;
    let mut query_rows = statement
        .query([])
        .with_context(|| format!("failed to query view `{}`", spec.view))?;
    let mut rows = Vec::new();
    while let Some(row) = query_rows
        .next()
        .with_context(|| format!("failed to read view `{}`", spec.view))?
    {
        let function = function_column
            .map(|column| row.get::<_, Value>(column))
            .transpose()
            .with_context(|| format!("failed to read view `{}`", spec.view))?
            .as_ref()
            .and_then(as_text)
            .map(str::to_string);
        let values = columns
            .iter()
            .map(|column| {
                let value = row.get::<_, Value>(column.key.as_str())?;
                Ok(format_value(&value, &column.format))
            })
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("failed to read view `{}`", spec.view))?;
        rows.push(MetricsRow {
            function,
            values,
            source: None,
        });
    }
    let source_locations = source_locations(connection).unwrap_or_default();
    for row in &mut rows {
        row.source = row
            .function
            .as_ref()
            .and_then(|function| source_locations.get(function))
            .cloned();
    }

    Ok(MetricsTableData {
        columns: columns.iter().map(display_column).collect(),
        rows,
        error: None,
    })
}

fn source_locations(connection: &Connection) -> Result<HashMap<String, SourceLocation>> {
    let query = "
        SELECT proc_map.func_name AS func_name,
               proc_map.file_name AS file_name,
               proc_map.line AS line,
               SUM(pmu_counters.pmu_cycles) AS total_cycles
        FROM pmu_counters
        INNER JOIN proc_map ON proc_map.ip = pmu_counters.ip
        WHERE proc_map.func_name IS NOT NULL
          AND proc_map.file_name IS NOT NULL
          AND proc_map.file_name <> ''
          AND proc_map.line IS NOT NULL
          AND proc_map.line > 0
        GROUP BY proc_map.func_name, proc_map.file_name, proc_map.line
        ORDER BY total_cycles DESC;
    ";
    let mut statement = connection
        .prepare(query)
        .context("failed to prepare source-location query")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .context("failed to read source-location query")?;
    let mut locations = HashMap::new();
    for row in rows {
        let (function, file, line) = row.context("failed to read source-location query")?;
        locations.entry(function).or_insert_with(|| SourceLocation {
            path: PathBuf::from(file),
            line: line.max(1) as usize,
        });
    }
    Ok(locations)
}

fn available_columns(connection: &Connection, view: &str) -> Result<HashSet<String>> {
    let columns = table_columns(connection, view)
        .into_iter()
        .map(|column| column.name)
        .collect::<HashSet<_>>();

    if columns.is_empty() {
        anyhow::bail!("view `{view}` does not exist or has no columns");
    }
    Ok(columns)
}

fn resolved_columns(
    spec: &MetricsTableSpec,
    available: &HashSet<String>,
) -> Result<Vec<ResolvedColumn>> {
    let mut configured = Vec::new();
    if spec.include_default_columns {
        configured.extend(default_columns());
    }
    configured.extend(spec.columns.iter().map(resolved_column));

    let mut missing = Vec::new();
    configured.retain(|column| {
        if available.contains(&column.key) {
            true
        } else {
            if !column_optional(spec, &column.key) {
                missing.push(column.label.clone());
            }
            false
        }
    });

    if !missing.is_empty() {
        anyhow::bail!("missing required columns: {}", missing.join(", "));
    }
    if configured.is_empty() {
        anyhow::bail!("no configured columns are available");
    }
    Ok(configured)
}

fn column_optional(spec: &MetricsTableSpec, key: &str) -> bool {
    spec.columns
        .iter()
        .find(|column| column.key == key)
        .is_some_and(|column| column.optional)
}

fn build_query(spec: &MetricsTableSpec, available: &HashSet<String>) -> String {
    let mut query = format!("SELECT * FROM {}", quote_identifier(&spec.view));
    if let Some(order) = spec
        .order_by
        .as_ref()
        .filter(|order| available.contains(&order.column))
    {
        query.push_str(" ORDER BY ");
        query.push_str(&quote_identifier(&order.column));
        query.push(' ');
        query.push_str(match order.direction {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        });
    }
    if let Some(limit) = spec.limit {
        query.push_str(&format!(" LIMIT {limit}"));
    }
    query
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn default_columns() -> Vec<ResolvedColumn> {
    vec![
        column("func_name", "Function", ValueFormat::Text, Some(52)),
        column("total", "Total %", ValueFormat::Percent2, Some(12)),
        column("cycles", "Cycles", ValueFormat::Integer, Some(18)),
        column(
            "instructions",
            "Instructions",
            ValueFormat::Integer,
            Some(18),
        ),
        column("ipc", "IPC", ValueFormat::Float2, Some(10)),
    ]
}

fn resolved_column(spec: &MetricColumnSpec) -> ResolvedColumn {
    column(
        &spec.key,
        spec.label.as_deref().unwrap_or(&spec.key),
        spec.format.clone(),
        spec.width,
    )
}

fn column(key: &str, label: &str, format: ValueFormat, width: Option<u16>) -> ResolvedColumn {
    ResolvedColumn {
        key: key.to_string(),
        label: label.to_string(),
        format,
        width,
    }
}

fn display_columns(spec: &MetricsTableSpec) -> Vec<MetricsColumn> {
    let mut columns = Vec::new();
    if spec.include_default_columns {
        columns.extend(default_columns());
    }
    columns.extend(spec.columns.iter().map(resolved_column));
    columns.iter().map(display_column).collect()
}

fn display_column(column: &ResolvedColumn) -> MetricsColumn {
    let align_right = column.format != ValueFormat::Text;
    let character_width = column.width.unwrap_or(if align_right { 16 } else { 36 }) as f32;
    MetricsColumn {
        label: column.label.clone(),
        width: (character_width * 8.0).max(if align_right { 80.0 } else { 280.0 }),
        align_right,
    }
}

fn format_value(value: &Value, format: &ValueFormat) -> String {
    match format {
        ValueFormat::Text => match value {
            Value::Float(_) | Value::Double(_) => as_f64(value)
                .map(|value| format_number(value, 2))
                .unwrap_or_else(|| "N/A".to_string()),
            _ => as_text(value).map(str::to_string).unwrap_or_else(|| {
                as_i64(value)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "N/A".to_string())
            }),
        },
        ValueFormat::Integer => as_i64(value)
            .map(|value| value.to_formatted_string(&Locale::en))
            .unwrap_or_else(|| "N/A".to_string()),
        ValueFormat::Float
        | ValueFormat::Float1
        | ValueFormat::Float2
        | ValueFormat::Float3
        | ValueFormat::Auto => as_f64(value)
            .map(|value| format_number(value, precision(format)))
            .unwrap_or_else(|| "N/A".to_string()),
        ValueFormat::Percent
        | ValueFormat::Percent1
        | ValueFormat::Percent2
        | ValueFormat::Percent3 => as_f64(value)
            .map(|value| format!("{}%", format_number(value * 100.0, precision(format))))
            .unwrap_or_else(|| "N/A".to_string()),
    }
}

fn precision(format: &ValueFormat) -> usize {
    match format {
        ValueFormat::Float1 | ValueFormat::Percent1 => 1,
        ValueFormat::Float3 | ValueFormat::Percent3 => 3,
        _ => 2,
    }
}

fn format_number(value: f64, precision: usize) -> String {
    format!("{value:.precision$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MetricsTableSpec {
        MetricsTableSpec {
            view: "hotspots".to_string(),
            title: Some("Hotspots".to_string()),
            include_default_columns: true,
            columns: Vec::new(),
            order_by: Some(pmu_data::OrderSpec {
                column: "total".to_string(),
                direction: SortDirection::Desc,
            }),
            limit: Some(2),
            sticky_columns: Some(1),
            function_column: Some("func_name".to_string()),
            enable_assembly: true,
        }
    }

    #[test]
    fn loads_and_formats_metrics_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE hotspots (
                    func_name TEXT, total DOUBLE, cycles BIGINT,
                    instructions BIGINT, ipc DOUBLE
                );
                INSERT INTO hotspots VALUES ('cold', 0.1, 10, 20, 2.0);
                INSERT INTO hotspots VALUES ('hot', 0.9, 90, 135, 1.5);",
            )
            .unwrap();

        let table = MetricsTableData::load(&connection, &spec());

        assert!(table.error.is_none());
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].values[0], "hot");
        assert_eq!(table.rows[0].values[1], "90.00%");
        assert_eq!(table.rows[0].values[2], "90");
    }

    #[test]
    fn reports_missing_view_in_the_tab() {
        let connection = Connection::open_in_memory().unwrap();
        let table = MetricsTableData::load(&connection, &spec());
        assert!(table.error.as_deref().unwrap().contains("does not exist"));
    }

    #[test]
    fn associates_hottest_recorded_source_location_with_function() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE hotspots (
                    func_name TEXT, total DOUBLE, cycles BIGINT,
                    instructions BIGINT, ipc DOUBLE
                );
                INSERT INTO hotspots VALUES ('hot', 1.0, 90, 135, 1.5);
                CREATE TABLE proc_map (
                    ip BIGINT, func_name TEXT, file_name TEXT, line BIGINT
                );
                CREATE TABLE pmu_counters (ip BIGINT, pmu_cycles BIGINT);
                INSERT INTO proc_map VALUES (1, 'hot', '/src/hot.rs', 12);
                INSERT INTO proc_map VALUES (2, 'hot', '/src/hot.rs', 44);
                INSERT INTO pmu_counters VALUES (1, 10);
                INSERT INTO pmu_counters VALUES (2, 80);",
            )
            .unwrap();

        let table = MetricsTableData::load(&connection, &spec());

        assert_eq!(table.rows[0].function.as_deref(), Some("hot"));
        assert_eq!(
            table.rows[0].source,
            Some(SourceLocation {
                path: PathBuf::from("/src/hot.rs"),
                line: 44,
            })
        );
    }
}
