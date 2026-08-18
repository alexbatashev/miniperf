use std::collections::BTreeMap;

use crate::sql::Connection;

/// USE-method snapshot data: per-resource utilization/saturation/errors
/// summaries, ranked findings, collector coverage, and metric time series.
#[derive(Debug, Clone)]
pub struct SnapshotData {
    pub resources: Vec<ResourceUse>,
    pub findings: Vec<SnapshotFinding>,
    pub collectors: Vec<SnapshotCollector>,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Medium,
    High,
}

impl Severity {
    fn parse(value: &str) -> Self {
        match value {
            "high" => Self::High,
            "medium" => Self::Medium,
            _ => Self::Info,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UseCategory {
    Utilization,
    Saturation,
    Errors,
}

impl UseCategory {
    pub const ALL: [Self; 3] = [Self::Utilization, Self::Saturation, Self::Errors];

    fn parse(value: &str) -> Option<Self> {
        match value {
            "utilization" => Some(Self::Utilization),
            "saturation" => Some(Self::Saturation),
            "errors" => Some(Self::Errors),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Utilization => "Utilization",
            Self::Saturation => "Saturation",
            Self::Errors => "Errors",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotFinding {
    pub severity: Severity,
    pub resource: String,
    pub finding: String,
    pub evidence: String,
    pub recommendation: String,
    pub quality: String,
}

#[derive(Debug, Clone)]
pub struct SnapshotCollector {
    pub name: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ResourceUse {
    pub resource: String,
    /// Derived one-line summary, e.g. "3.5% of 48 CPUs".
    pub headline: Option<String>,
    pub severity: Severity,
    /// Formatted summary metrics per USE category, in category order.
    pub summaries: [Vec<SummaryMetric>; 3],
    pub charts: Vec<SnapshotChart>,
}

#[derive(Debug, Clone)]
pub struct SummaryMetric {
    pub metric: String,
    pub value: String,
    pub scope: String,
    /// 0..=1 position against the metric's natural ceiling, when it has one.
    pub fraction: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SnapshotChart {
    pub metric: String,
    pub category: UseCategory,
    /// Display unit after rate conversion, e.g. "MiB/s", "cores", "%".
    pub unit: String,
    /// One series per resource id (host, device, interface, …), sorted by id.
    pub series: Vec<ChartSeries>,
    pub max_value: f64,
}

#[derive(Debug, Clone)]
pub struct ChartSeries {
    pub id: String,
    /// (seconds from recording start, value in display units)
    pub points: Vec<(f64, f64)>,
}

struct SampleRow {
    timestamp_ns: u64,
    resource: String,
    resource_id: String,
    category: UseCategory,
    metric: String,
    value: f64,
    unit: String,
    scope: String,
}

impl SnapshotData {
    /// Returns `None` when the recording carries no usable snapshot tables.
    pub fn load(connection: &Connection, logical_cpu_count: Option<u32>) -> Option<Self> {
        let samples = load_samples(connection)?;
        if samples.is_empty() {
            return None;
        }
        let duration_ns = samples
            .iter()
            .map(|sample| sample.timestamp_ns)
            .max()
            .unwrap_or(0);
        let findings = load_findings(connection);
        let collectors = load_collectors(connection);
        let summary_rows = load_summary(connection);
        let resources = build_resources(
            samples,
            &summary_rows,
            &findings,
            duration_ns,
            logical_cpu_count,
        );
        if resources.is_empty() {
            return None;
        }

        Some(Self {
            resources,
            findings,
            collectors,
            duration_ns,
        })
    }
}

fn load_samples(connection: &Connection) -> Option<Vec<SampleRow>> {
    let mut statement = connection
        .prepare(
            "SELECT timestamp_ns, resource, resource_id, category, metric, value, unit, scope
             FROM snapshot_resource_samples ORDER BY timestamp_ns ASC;",
        )
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .ok()?;
    let mut samples = Vec::new();
    for row in rows {
        let (timestamp_ns, resource, resource_id, category, metric, value, unit, scope) =
            row.ok()?;
        let Some(category) = UseCategory::parse(&category) else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        samples.push(SampleRow {
            timestamp_ns: timestamp_ns.max(0) as u64,
            resource,
            resource_id,
            category,
            metric,
            value,
            unit,
            scope,
        });
    }
    Some(samples)
}

struct SummaryRow {
    resource: String,
    category: UseCategory,
    metric: String,
    value: Option<f64>,
    unit: String,
    scope: String,
}

fn load_summary(connection: &Connection) -> Vec<SummaryRow> {
    let Ok(mut statement) = connection.prepare(
        "SELECT resource, category, metric, value, unit, scope FROM snapshot_summary
         ORDER BY resource, category, metric;",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<f64>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.filter_map(|row| {
        let (resource, category, metric, value, unit, scope) = row.ok()?;
        Some(SummaryRow {
            resource,
            category: UseCategory::parse(&category)?,
            metric,
            value,
            unit,
            scope,
        })
    })
    .collect()
}

fn load_findings(connection: &Connection) -> Vec<SnapshotFinding> {
    let Ok(mut statement) = connection.prepare(
        "SELECT severity, resource, finding, evidence, recommendation, quality
         FROM snapshot_findings ORDER BY \"rank\" ASC;",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok(SnapshotFinding {
            severity: Severity::parse(&row.get::<_, String>(0)?),
            resource: row.get(1)?,
            finding: row.get(2)?,
            evidence: row.get(3)?,
            recommendation: row.get(4)?,
            quality: row.get(5)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(|row| row.ok()).collect()
}

fn load_collectors(connection: &Connection) -> Vec<SnapshotCollector> {
    let Ok(mut statement) = connection
        .prepare("SELECT name, status, message FROM snapshot_collectors ORDER BY name;")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok(SnapshotCollector {
            name: row.get(0)?,
            status: row.get(1)?,
            message: row.get(2)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(|row| row.ok()).collect()
}

const RESOURCE_ORDER: [&str; 5] = ["cpu", "memory", "disk", "io", "network"];

fn build_resources(
    samples: Vec<SampleRow>,
    summary_rows: &[SummaryRow],
    findings: &[SnapshotFinding],
    duration_ns: u64,
    logical_cpu_count: Option<u32>,
) -> Vec<ResourceUse> {
    let mut grouped =
        BTreeMap::<(String, UseCategory, String), BTreeMap<String, Vec<(u64, f64)>>>::new();
    let mut units = BTreeMap::<(String, UseCategory, String), (String, String)>::new();
    for sample in samples {
        let key = (sample.resource, sample.category, sample.metric);
        units
            .entry(key.clone())
            .or_insert((sample.unit, sample.scope));
        grouped
            .entry(key)
            .or_default()
            .entry(sample.resource_id)
            .or_default()
            .push((sample.timestamp_ns, sample.value));
    }

    let mut charts_by_resource = BTreeMap::<String, Vec<SnapshotChart>>::new();
    for ((resource, category, metric), by_id) in grouped {
        let (unit, _) = units
            .remove(&(resource.clone(), category, metric.clone()))
            .unwrap_or_default();
        let kind = metric_kind(&metric, &unit);
        let mut series = by_id
            .into_iter()
            .filter_map(|(id, points)| build_series(id, points, kind, &unit))
            .collect::<Vec<_>>();
        if series.is_empty() {
            continue;
        }
        series.sort_by(|left, right| left.id.cmp(&right.id));
        let max_value = series
            .iter()
            .flat_map(|series| series.points.iter().map(|point| point.1))
            .fold(0.0_f64, f64::max);
        charts_by_resource
            .entry(resource)
            .or_default()
            .push(SnapshotChart {
                metric,
                category,
                unit: kind.display_unit(&unit),
                series,
                max_value,
            });
    }
    for charts in charts_by_resource.values_mut() {
        charts.sort_by(|left, right| {
            left.category
                .cmp(&right.category)
                .then_with(|| left.metric.cmp(&right.metric))
        });
    }

    let mut severities = BTreeMap::<&str, Severity>::new();
    for finding in findings {
        let entry = severities
            .entry(&finding.resource)
            .or_insert(Severity::Info);
        *entry = (*entry).max(finding.severity);
    }

    let mut resources = charts_by_resource.keys().cloned().collect::<Vec<_>>();
    resources.sort_by_key(|resource| {
        RESOURCE_ORDER
            .iter()
            .position(|known| known == resource)
            .unwrap_or(RESOURCE_ORDER.len())
    });

    resources
        .into_iter()
        .map(|resource| {
            let charts = charts_by_resource.remove(&resource).unwrap_or_default();
            let summaries = summarize(summary_rows, &resource, duration_ns, logical_cpu_count);
            ResourceUse {
                headline: headline(
                    &resource,
                    &charts,
                    summary_rows,
                    duration_ns,
                    logical_cpu_count,
                ),
                severity: severities
                    .get(resource.as_str())
                    .copied()
                    .unwrap_or(Severity::Info),
                summaries,
                charts,
                resource,
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum MetricKind {
    Gauge,
    /// Monotone counter charted as a per-second rate.
    Rate,
    /// Cumulative busy/CPU time charted as occupancy (time per wall time).
    Occupancy,
}

impl MetricKind {
    fn display_unit(self, unit: &str) -> String {
        match self {
            Self::Gauge => unit.to_string(),
            Self::Rate => format!("{unit}/s"),
            Self::Occupancy if unit == "seconds" => "cores".to_string(),
            Self::Occupancy => "%".to_string(),
        }
    }
}

fn metric_kind(metric: &str, unit: &str) -> MetricKind {
    match unit {
        "seconds" | "milliseconds" => MetricKind::Occupancy,
        "faults" | "switches" | "events" | "operations" => MetricKind::Rate,
        "bytes"
            if ["read", "write", "receive", "transmit", "dram"]
                .iter()
                .any(|prefix| metric.contains(prefix)) =>
        {
            MetricKind::Rate
        }
        _ => MetricKind::Gauge,
    }
}

fn build_series(
    id: String,
    points: Vec<(u64, f64)>,
    kind: MetricKind,
    unit: &str,
) -> Option<ChartSeries> {
    // Milliseconds of busy time per second of wall time become percent;
    // seconds of CPU time per second of wall time stay as cores.
    let scale = match kind {
        MetricKind::Occupancy if unit == "milliseconds" => 0.1,
        _ => 1.0,
    };
    let converted: Vec<(f64, f64)> = match kind {
        MetricKind::Gauge => points
            .iter()
            .map(|(timestamp, value)| (*timestamp as f64 / 1e9, *value))
            .collect(),
        MetricKind::Rate | MetricKind::Occupancy => points
            .windows(2)
            .filter_map(|pair| {
                let dt = (pair[1].0 as f64 - pair[0].0 as f64) / 1e9;
                if dt <= 0.0 {
                    return None;
                }
                // Counter resets chart as zero instead of a negative spike.
                let delta = (pair[1].1 - pair[0].1).max(0.0);
                Some((pair[1].0 as f64 / 1e9, delta / dt * scale))
            })
            .collect(),
    };
    if converted.is_empty() {
        return None;
    }
    Some(ChartSeries {
        id,
        points: converted,
    })
}

fn summarize(
    summary_rows: &[SummaryRow],
    resource: &str,
    duration_ns: u64,
    logical_cpu_count: Option<u32>,
) -> [Vec<SummaryMetric>; 3] {
    let host_total = summary_value(summary_rows, resource, "host_total");
    let mut result: [Vec<SummaryMetric>; 3] = Default::default();
    for row in summary_rows.iter().filter(|row| row.resource == resource) {
        let index = UseCategory::ALL
            .iter()
            .position(|category| *category == row.category)
            .unwrap_or(0);
        result[index].push(SummaryMetric {
            metric: row.metric.clone(),
            value: row
                .value
                .map(|value| format_value(value, &row.unit))
                .unwrap_or_else(|| "—".to_string()),
            scope: row.scope.clone(),
            fraction: metric_fraction(row, duration_ns, logical_cpu_count, host_total),
        });
    }
    result
}

/// Normalizes a summary value into 0..=1 when its unit has a natural ceiling:
/// percentages, CPU time against the machine's capacity, byte gauges against
/// the host's total. Everything else has no meaningful meter.
fn metric_fraction(
    row: &SummaryRow,
    duration_ns: u64,
    logical_cpu_count: Option<u32>,
    host_total: Option<f64>,
) -> Option<f64> {
    let value = row.value?;
    let fraction = match row.unit.as_str() {
        "percent" => value / 100.0,
        "seconds" if row.resource == "cpu" => {
            let capacity =
                (duration_ns as f64 / 1e9) * logical_cpu_count.unwrap_or(1).max(1) as f64;
            value / capacity
        }
        "bytes" => value / host_total.filter(|total| *total > 0.0)?,
        _ => return None,
    };
    fraction.is_finite().then(|| fraction.clamp(0.0, 1.0))
}

fn summary_value(summary_rows: &[SummaryRow], resource: &str, metric: &str) -> Option<f64> {
    summary_rows
        .iter()
        .find(|row| row.resource == resource && row.metric == metric)
        .and_then(|row| row.value)
}

fn headline(
    resource: &str,
    charts: &[SnapshotChart],
    summary_rows: &[SummaryRow],
    duration_ns: u64,
    logical_cpu_count: Option<u32>,
) -> Option<String> {
    let duration_s = (duration_ns as f64 / 1e9).max(f64::EPSILON);
    match resource {
        "cpu" => {
            let cpu_s = summary_value(summary_rows, "cpu", "cgroup_cpu_time")
                .filter(|v| *v > 0.0)
                .or_else(|| {
                    Some(
                        summary_value(summary_rows, "cpu", "user_time").unwrap_or(0.0)
                            + summary_value(summary_rows, "cpu", "system_time").unwrap_or(0.0),
                    )
                })?;
            let logical = logical_cpu_count.unwrap_or(1).max(1) as f64;
            Some(format!(
                "{:.1}% of {} CPUs · {:.2} cores average",
                cpu_s / duration_s / logical * 100.0,
                logical as u64,
                cpu_s / duration_s
            ))
        }
        "memory" => {
            let rss = summary_value(summary_rows, "memory", "pss")
                .into_iter()
                .chain(summary_value(summary_rows, "memory", "rss"))
                .fold(0.0_f64, f64::max);
            let host_total = summary_value(summary_rows, "memory", "host_total").unwrap_or(0.0);
            (rss > 0.0).then(|| {
                if host_total > 0.0 {
                    format!(
                        "peak tree {} · {:.1}% of host {}",
                        format_value(rss, "bytes"),
                        rss / host_total * 100.0,
                        format_value(host_total, "bytes")
                    )
                } else {
                    format!("peak tree {}", format_value(rss, "bytes"))
                }
            })
        }
        "disk" => {
            let busiest = charts
                .iter()
                .find(|chart| chart.metric == "busy_time")
                .and_then(|chart| {
                    chart
                        .series
                        .iter()
                        .map(|series| {
                            (
                                series.id.as_str(),
                                series
                                    .points
                                    .iter()
                                    .map(|point| point.1)
                                    .fold(0.0_f64, f64::max),
                            )
                        })
                        .max_by(|left, right| left.1.total_cmp(&right.1))
                });
            busiest.map(|(device, busy)| format!("peak device busy {busy:.1}% ({device})"))
        }
        "network" => {
            let received = summary_value(summary_rows, "network", "receive_bytes").unwrap_or(0.0);
            let transmitted =
                summary_value(summary_rows, "network", "transmit_bytes").unwrap_or(0.0);
            (received + transmitted > 0.0).then(|| {
                format!(
                    "{}/s in · {}/s out average",
                    format_value(received / duration_s, "bytes"),
                    format_value(transmitted / duration_s, "bytes")
                )
            })
        }
        "io" => {
            let psi = summary_rows
                .iter()
                .filter(|row| row.resource == "io" && row.metric.contains("psi_some"))
                .filter_map(|row| row.value)
                .fold(0.0_f64, f64::max);
            Some(format!("peak I/O pressure {psi:.1}% (PSI some avg10)"))
        }
        _ => None,
    }
}

/// Formats a raw metric value using its recorded unit.
pub fn format_value(value: f64, unit: &str) -> String {
    match unit {
        "bytes" => format_bytes(value),
        "bits_per_second" => format!("{:.1} Gbit/s", value / 1e9),
        "percent" => format!("{value:.1}%"),
        "seconds" => format!("{value:.1} s"),
        "milliseconds" => {
            if value >= 1_000.0 {
                format!("{:.1} s", value / 1_000.0)
            } else {
                format!("{value:.0} ms")
            }
        }
        _ => format_count(value),
    }
}

pub fn format_bytes(value: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = value;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_count(value: f64) -> String {
    if value.abs() >= 1e9 {
        format!("{:.2}G", value / 1e9)
    } else if value.abs() >= 1e6 {
        format!("{:.2}M", value / 1e6)
    } else if value.abs() >= 1e3 {
        format!("{:.1}k", value / 1e3)
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE snapshot_resource_samples (
                    timestamp_ns BIGINT, resource TEXT, resource_id TEXT, category TEXT,
                    metric TEXT, value DOUBLE, unit TEXT, scope TEXT, source TEXT, quality TEXT
                );
                CREATE TABLE snapshot_summary (
                    resource TEXT, category TEXT, metric TEXT, value DOUBLE,
                    unit TEXT, scope TEXT, source TEXT, quality TEXT
                );
                CREATE TABLE snapshot_findings (
                    \"rank\" BIGINT, severity TEXT, resource TEXT, finding TEXT,
                    evidence TEXT, recommendation TEXT, scope TEXT, quality TEXT
                );
                CREATE TABLE snapshot_collectors (
                    name TEXT, status TEXT, source TEXT, quality TEXT, message TEXT
                );
                ",
            )
            .unwrap();
        connection
    }

    struct Metric {
        resource: &'static str,
        id: &'static str,
        category: &'static str,
        metric: &'static str,
        unit: &'static str,
    }

    fn insert_sample(connection: &Connection, timestamp_s: u64, m: &Metric, value: f64) {
        let Metric {
            resource,
            id,
            category,
            metric,
            unit,
        } = m;
        connection
            .execute_batch(&format!(
                "INSERT INTO snapshot_resource_samples VALUES
                 ({}, '{resource}', '{id}', '{category}', '{metric}', {value}, '{unit}',
                  'process_tree', 'procfs', 'best_effort');",
                timestamp_s * 1_000_000_000
            ))
            .unwrap();
    }

    #[test]
    fn converts_counters_to_rates_and_keeps_gauges() {
        let connection = connection();
        const CPU: Metric = Metric {
            resource: "cpu",
            id: "process_tree",
            category: "utilization",
            metric: "cgroup_cpu_time",
            unit: "seconds",
        };
        const RSS: Metric = Metric {
            resource: "memory",
            id: "process_tree",
            category: "utilization",
            metric: "rss",
            unit: "bytes",
        };
        const DISK: Metric = Metric {
            resource: "disk",
            id: "sda",
            category: "utilization",
            metric: "busy_time",
            unit: "milliseconds",
        };
        for (t, cpu_s, rss, busy_ms) in [
            (0, 0.0, 100.0, 0.0),
            (1, 2.0, 200.0, 500.0),
            (2, 4.0, 150.0, 1500.0),
        ] {
            insert_sample(&connection, t, &CPU, cpu_s);
            insert_sample(&connection, t, &RSS, rss);
            insert_sample(&connection, t, &DISK, busy_ms);
        }
        connection
            .execute_batch(
                "INSERT INTO snapshot_summary VALUES
                 ('cpu', 'utilization', 'cgroup_cpu_time', 4.0, 'seconds', 'process_tree', 'cgroup_v2', 'exact');
                 INSERT INTO snapshot_findings VALUES
                 (1, 'high', 'memory', 'finding', 'evidence', 'recommendation', 'mixed', 'exact');
                 INSERT INTO snapshot_collectors VALUES
                 ('bpf', 'permission_denied', 'bpftrace', 'unavailable', 'no perms');",
            )
            .unwrap();

        let data = SnapshotData::load(&connection, Some(8)).unwrap();

        assert_eq!(
            data.resources
                .iter()
                .map(|r| r.resource.as_str())
                .collect::<Vec<_>>(),
            ["cpu", "memory", "disk"]
        );
        let cpu = &data.resources[0];
        // 4 CPU-seconds over 2 wall seconds on 8 CPUs → 25% · 2 cores.
        assert_eq!(
            cpu.headline.as_deref(),
            Some("25.0% of 8 CPUs · 2.00 cores average")
        );
        let cpu_chart = &cpu.charts[0];
        assert_eq!(cpu_chart.unit, "cores");
        assert_eq!(cpu_chart.series[0].points, vec![(1.0, 2.0), (2.0, 2.0)]);

        let memory = &data.resources[1];
        assert_eq!(memory.severity, Severity::High);
        let rss_chart = memory.charts.iter().find(|c| c.metric == "rss").unwrap();
        assert_eq!(rss_chart.unit, "bytes");
        assert_eq!(rss_chart.series[0].points.len(), 3);

        let disk_chart = &data.resources[2].charts[0];
        // 1000 ms of busy time in 1 s of wall time → 100%.
        assert_eq!(disk_chart.unit, "%");
        assert_eq!(disk_chart.series[0].points, vec![(1.0, 50.0), (2.0, 100.0)]);

        assert_eq!(data.findings.len(), 1);
        assert_eq!(data.collectors[0].status, "permission_denied");
        assert_eq!(data.duration_ns, 2_000_000_000);
    }

    #[test]
    fn summary_metrics_meter_only_against_a_real_ceiling() {
        let row = |resource: &str, metric: &str, value: f64, unit: &str| SummaryRow {
            resource: resource.to_owned(),
            category: UseCategory::Utilization,
            metric: metric.to_owned(),
            value: Some(value),
            unit: unit.to_owned(),
            scope: "process_tree".to_owned(),
        };
        let rows = [
            row("cpu", "cgroup_cpu_time", 4.0, "seconds"),
            row("memory", "rss", 2.0, "bytes"),
            row("memory", "host_total", 8.0, "bytes"),
            row("io", "psi_some_avg10", 25.0, "percent"),
            row("disk", "read_operations", 1234.0, "operations"),
        ];
        let fraction = |resource: &str| {
            summarize(&rows, resource, 2_000_000_000, Some(8))[0]
                .first()
                .and_then(|metric| metric.fraction)
        };

        // 4 CPU-seconds of a 2s × 8-CPU budget.
        assert_eq!(fraction("cpu"), Some(0.25));
        assert_eq!(fraction("memory"), Some(0.25));
        assert_eq!(fraction("io"), Some(0.25));
        assert_eq!(fraction("disk"), None);
    }

    #[test]
    fn missing_tables_yield_no_snapshot() {
        let connection = Connection::open_in_memory().unwrap();
        assert!(SnapshotData::load(&connection, None).is_none());
        let empty = self::tests::connection();
        assert!(SnapshotData::load(&empty, None).is_none());
    }
}
