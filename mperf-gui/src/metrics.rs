use std::collections::HashSet;

use anyhow::{Context, Result};
use num_format::{Locale, ToFormattedString};
use pmu_data::{MetricColumnSpec, MetricsTableSpec, SortDirection, ValueFormat};
use sqlite::{Connection, Value};

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
    let statement = connection
        .prepare(query)
        .with_context(|| format!("failed to query SQLite view `{}`", spec.view))?;
    let rows = statement
        .into_iter()
        .map(|row| {
            let row = row.with_context(|| format!("failed to read SQLite view `{}`", spec.view))?;
            Ok(MetricsRow {
                values: columns
                    .iter()
                    .map(|column| {
                        let value = row[column.key.as_str()].clone();
                        format_value(&value, &column.format)
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(MetricsTableData {
        columns: columns.iter().map(display_column).collect(),
        rows,
        error: None,
    })
}

fn available_columns(connection: &Connection, view: &str) -> Result<HashSet<String>> {
    let escaped_view = view.replace('\'', "''");
    let query = format!("PRAGMA table_info('{escaped_view}')");
    let columns = connection
        .prepare(query)
        .with_context(|| format!("failed to inspect SQLite view `{view}`"))?
        .into_iter()
        .map(|row| {
            let row = row.with_context(|| format!("failed to inspect SQLite view `{view}`"))?;
            Ok(row.read::<&str, _>("name").to_string())
        })
        .collect::<Result<HashSet<_>>>()?;

    if columns.is_empty() {
        anyhow::bail!("SQLite view `{view}` does not exist or has no columns");
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
            Value::String(value) => value.clone(),
            Value::Integer(value) => value.to_string(),
            Value::Float(value) => format_number(*value, 2),
            _ => "N/A".to_string(),
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

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Float(value) => Some(*value as i64),
        _ => None,
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Float(value) => Some(*value),
        Value::Integer(value) => Some(*value as f64),
        _ => None,
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
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE hotspots (
                    func_name TEXT, total REAL, cycles INTEGER,
                    instructions INTEGER, ipc REAL
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
        let connection = sqlite::open(":memory:").unwrap();
        let table = MetricsTableData::load(&connection, &spec());
        assert!(table.error.as_deref().unwrap().contains("does not exist"));
    }
}
