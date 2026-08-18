use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use mperf_data::{RecordInfo, ScenarioInfo};

use super::tables::{Columns, Tables};

/// Materialize the snapshot tables from the procfs and bpftrace side files.
pub(crate) fn process(tables: &Tables, info: &RecordInfo, res_dir: &Path) -> Result<()> {
    write_resource_samples(tables, res_dir)?;
    write_processes(tables, res_dir)?;
    write_collectors(tables, info)?;
    tables.write_query(
        "snapshot_summary",
        "SELECT resource, category, metric, MAX(value) AS value, unit, scope, source, quality
         FROM snapshot_resource_samples
         GROUP BY resource, category, metric, unit, scope, source, quality",
    )?;
    write_findings(tables, info)?;
    remove_intermediates(tables, res_dir)
}

/// Delete the record-time intermediates once the final snapshot tables are
/// materialized, mirroring `samples_raw`.
fn remove_intermediates(tables: &Tables, res_dir: &Path) -> Result<()> {
    tables.connection().execute_batch(
        "DROP VIEW IF EXISTS resource_samples; DROP VIEW IF EXISTS process_samples;",
    )?;
    for entry in std::fs::read_dir(res_dir)? {
        let path = entry?.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.extension().is_some_and(|ext| ext == "parquet")
            && matches!(
                store::table_base_name(stem),
                "resource_samples" | "process_samples"
            )
        {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn write_resource_samples(tables: &Tables, _res_dir: &Path) -> Result<()> {
    let mut timestamp_ns = Vec::new();
    let mut resource = Vec::new();
    let mut resource_id = Vec::new();
    let mut category = Vec::new();
    let mut metric = Vec::new();
    let mut value = Vec::new();
    let mut unit = Vec::new();
    let mut scope = Vec::new();
    let mut source = Vec::new();
    let mut quality = Vec::new();

    if tables.has_table("resource_samples") {
        let mut statement = tables.connection().prepare(
            "SELECT timestamp_ns, resource, resource_id, category, metric, value, unit, \
             scope, source, quality FROM resource_samples ORDER BY timestamp_ns",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            timestamp_ns.push(row.get(0)?);
            resource.push(row.get(1)?);
            resource_id.push(row.get(2)?);
            category.push(row.get(3)?);
            metric.push(row.get(4)?);
            value.push(row.get(5)?);
            unit.push(row.get(6)?);
            scope.push(row.get(7)?);
            source.push(row.get(8)?);
            quality.push(row.get(9)?);
        }
    }

    if tables.has_table("bpf_metrics") {
        let mut statement = tables
            .connection()
            .prepare("SELECT metric, value FROM bpf_metrics")?;
        let values = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?
            .collect::<store::duckdb::Result<HashMap<_, _>>>()?;
        let latest = timestamp_ns.iter().copied().max().unwrap_or(0);
        for (bpf_resource, bpf_category, bpf_metric, bpf_unit, bpf_value) in [
            (
                "cpu",
                "saturation",
                "run_queue_latency",
                "nanoseconds",
                values.get("runq_ns").copied().unwrap_or(0.0)
                    / values.get("runq_count").copied().unwrap_or(0.0).max(1.0),
            ),
            (
                "disk",
                "saturation",
                "block_latency",
                "nanoseconds",
                values.get("block_latency_ns").copied().unwrap_or(0.0)
                    / values.get("block_count").copied().unwrap_or(0.0).max(1.0),
            ),
            (
                "disk",
                "utilization",
                "block_bytes",
                "bytes",
                values.get("block_bytes").copied().unwrap_or(0.0),
            ),
            (
                "network",
                "errors",
                "tcp_retransmits",
                "events",
                values.get("tcp_retransmits").copied().unwrap_or(0.0),
            ),
        ] {
            timestamp_ns.push(latest);
            resource.push(bpf_resource.to_owned());
            resource_id.push("process_tree".to_owned());
            category.push(bpf_category.to_owned());
            metric.push(bpf_metric.to_owned());
            value.push(bpf_value);
            unit.push(bpf_unit.to_owned());
            scope.push("process_tree".to_owned());
            source.push("bpftrace".to_owned());
            quality.push("attributed".to_owned());
        }
    }

    let mut columns = Columns::default();
    columns.i64("timestamp_ns", timestamp_ns);
    columns.text("resource", resource);
    columns.text("resource_id", resource_id);
    columns.text("category", category);
    columns.text("metric", metric);
    columns.f64("value", value);
    columns.text("unit", unit);
    columns.text("scope", scope);
    columns.text("source", source);
    columns.text("quality", quality);
    tables.write("snapshot_resource_samples", columns.finish()?)
}

fn write_processes(tables: &Tables, _res_dir: &Path) -> Result<()> {
    if tables.has_table("process_samples") {
        return tables.write_query(
            "snapshot_processes",
            "SELECT pid, ppid, start_ticks, first_seen_ns, last_seen_ns, command, quality \
             FROM process_samples ORDER BY first_seen_ns, pid",
        );
    }
    let mut columns = Columns::default();
    columns.i64("pid", Vec::new());
    columns.i64("ppid", Vec::new());
    columns.i64("start_ticks", Vec::new());
    columns.i64("first_seen_ns", Vec::new());
    columns.i64("last_seen_ns", Vec::new());
    columns.text("command", Vec::new());
    columns.text("quality", Vec::new());
    tables.write("snapshot_processes", columns.finish()?)
}

fn write_collectors(tables: &Tables, info: &RecordInfo) -> Result<()> {
    let mut name = Vec::new();
    let mut status = Vec::new();
    let mut source = Vec::new();
    let mut quality = Vec::new();
    let mut message = Vec::new();
    if let ScenarioInfo::Snapshot(snapshot) = &info.scenario_info {
        for collector in &snapshot.collectors {
            name.push(collector.name.clone());
            status.push(collector.status.clone());
            source.push(collector.source.clone());
            quality.push(collector.quality.clone());
            message.push(collector.message.clone());
        }
    }

    let mut columns = Columns::default();
    columns.text("name", name);
    columns.text("status", status);
    columns.text("source", source);
    columns.text("quality", quality);
    columns.text("message", message);
    tables.write("snapshot_collectors", columns.finish()?)
}

fn write_findings(tables: &Tables, info: &RecordInfo) -> Result<()> {
    let duration_ns = tables
        .scalar_f64("SELECT MAX(timestamp_ns) FROM snapshot_resource_samples")
        .unwrap_or(0.0)
        .max(1.0);
    let duration_s = duration_ns / 1_000_000_000.0;
    let cgroup_cpu_s = max_metric(tables, "cgroup_cpu_time");
    let cpu_s = if cgroup_cpu_s > 0.0 {
        cgroup_cpu_s
    } else {
        max_metric(tables, "user_time") + max_metric(tables, "system_time")
    };
    let logical = info.logical_cpu_count.unwrap_or(1).max(1) as f64;
    let cpu_percent = cpu_s / duration_s / logical * 100.0;
    let cgroup_memory = max_metric(tables, "cgroup_memory_peak");
    let rss = if cgroup_memory > 0.0 {
        cgroup_memory
    } else {
        max_metric(tables, "pss").max(max_metric(tables, "rss"))
    };
    let host_total = max_metric(tables, "host_total");
    let cgroup_limit = max_metric(tables, "cgroup_memory_limit");
    let memory_capacity = if cgroup_limit > 0.0 {
        if host_total > 0.0 {
            cgroup_limit.min(host_total)
        } else {
            cgroup_limit
        }
    } else {
        host_total
    };
    let memory_percent = if memory_capacity > 0.0 {
        rss / memory_capacity * 100.0
    } else {
        0.0
    };
    let cpu_psi =
        max_metric(tables, "cgroup_psi_some_avg10").max(max_metric(tables, "psi_some_avg10"));
    let major_faults = max_metric(tables, "major_faults");
    let oom_kills = max_metric(tables, "oom_kills");
    let read_gbps = rate_metric(tables, "dram_read_bytes", duration_s) / 1e9;
    let write_gbps = rate_metric(tables, "dram_write_bytes", duration_s) / 1e9;
    let network_errors = metric_delta_sum(
        tables,
        &[
            "receive_errors",
            "receive_drops",
            "transmit_errors",
            "transmit_drops",
        ],
    );
    let tcp_retransmits = max_metric(tables, "tcp_retransmits");
    let network_utilization = max_network_utilization(tables, duration_s)?;
    let disk_busy = max_device_rate(tables, "busy_time", duration_s * 1_000.0) * 100.0;
    let tree_quality = if cgroup_cpu_s > 0.0 {
        "exact_process_tree"
    } else {
        "best_effort"
    };

    let mut findings = Vec::<(i64, &str, &str, String, String, String, &str, &str)>::new();
    if cpu_percent >= 85.0 || cpu_psi >= 5.0 {
        findings.push((1, "high", "cpu", "CPU capacity or run queue pressure is high".into(), format!("tree CPU utilization {cpu_percent:.1}%; CPU PSI {cpu_psi:.1}%"), "Inspect the Hotspots view, then run the TMA scenario or `perf sched timehist` to separate pipeline stalls from scheduling delay.".into(), "mixed", tree_quality));
    } else {
        findings.push((10, "info", "cpu", "CPU capacity has headroom".into(), format!("tree CPU utilization {cpu_percent:.1}%; CPU PSI {cpu_psi:.1}%"), "Use Hotspots to confirm where CPU time is spent before enabling a higher-frequency profile.".into(), "process_tree", tree_quality));
    }
    if memory_percent >= 85.0 || major_faults > 0.0 || oom_kills > 0.0 {
        findings.push((2, "high", "memory", "Memory capacity or paging pressure is visible".into(), format!("peak tree memory {:.1} MiB ({memory_percent:.1}% of host); major faults {major_faults:.0}; OOM kills {oom_kills:.0}", rss / 1048576.0), "Run `mperf record --scenario mem` and inspect allocation lifetime, working set, locality, and DRAM traffic.".into(), "mixed", if cgroup_memory > 0.0 { "exact_process_tree" } else { "best_effort" }));
    }
    if read_gbps + write_gbps > 0.0 {
        findings.push((20, "info", "memory", "System DRAM traffic was measurable".into(), format!("average {:.2} GB/s read and {:.2} GB/s write", read_gbps, write_gbps), "Treat this as system-scoped. Run the memory scenario when workload-specific traffic and a sustainable bandwidth comparison are needed.".into(), "system_during_target", "exact_system"));
    }
    if disk_busy >= 80.0 {
        findings.push((3, "high", "disk", "A block device was highly utilized".into(), format!("maximum device busy estimate {disk_busy:.1}%"), "Use `iostat -xz 1` and BPF block latency tracing (`biolatency`/`biosnoop`) to identify the device and I/O path.".into(), "system_during_target", "exact_system"));
    }
    if network_utilization >= 70.0 || network_errors > 0.0 || tcp_retransmits > 0.0 {
        findings.push((4, "medium", "network", "Network utilization or reliability needs attention".into(), format!("maximum known-link utilization {network_utilization:.1}%; {network_errors:.0} host interface errors/drops; {tcp_retransmits:.0} attributed TCP retransmits"), "Inspect `ip -s link`, `ss -ti`, retransmits, and then capture packets on the affected interface if needed.".into(), "mixed", if tcp_retransmits > 0.0 { "attributed" } else { "exact_system" }));
    }

    let mut unavailable = tables.connection().prepare(
        "SELECT name, status, message FROM snapshot_collectors WHERE status <> 'available'",
    )?;
    let degraded = unavailable
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<store::duckdb::Result<Vec<_>>>()?;
    for (name, state, message) in degraded {
        findings.push((90, "info", "coverage", format!("Collector {name} is {state}"), message, "Use the recorded fallback data or grant the documented kernel capabilities before repeating a latency-attributed snapshot.".into(), "collector", "unavailable"));
    }
    findings.sort_by_key(|finding| finding.0);

    let mut columns = Columns::default();
    columns.i64("rank", (1..=findings.len() as i64).collect::<Vec<_>>());
    columns.text(
        "severity",
        findings.iter().map(|f| f.1.to_owned()).collect(),
    );
    columns.text(
        "resource",
        findings.iter().map(|f| f.2.to_owned()).collect(),
    );
    columns.text("finding", findings.iter().map(|f| f.3.clone()).collect());
    columns.text("evidence", findings.iter().map(|f| f.4.clone()).collect());
    columns.text(
        "recommendation",
        findings.iter().map(|f| f.5.clone()).collect(),
    );
    columns.text("scope", findings.iter().map(|f| f.6.to_owned()).collect());
    columns.text("quality", findings.iter().map(|f| f.7.to_owned()).collect());
    tables.write("snapshot_findings", columns.finish()?)
}

fn max_metric(tables: &Tables, metric: &str) -> f64 {
    let metric = metric.replace('\'', "''");
    tables
        .scalar_f64(&format!(
            "SELECT COALESCE(MAX(value), 0) FROM snapshot_resource_samples WHERE metric = '{metric}'"
        ))
        .unwrap_or(0.0)
}

fn rate_metric(tables: &Tables, metric: &str, duration_s: f64) -> f64 {
    let metric = metric.replace('\'', "''");
    tables
        .scalar_f64(&format!(
            "SELECT COALESCE(MAX(value) - MIN(value), 0) FROM snapshot_resource_samples
             WHERE metric = '{metric}'"
        ))
        .unwrap_or(0.0)
        / duration_s.max(f64::EPSILON)
}

fn metric_delta_sum(tables: &Tables, metrics: &[&str]) -> f64 {
    metrics
        .iter()
        .map(|metric| {
            let metric = metric.replace('\'', "''");
            tables
                .scalar_f64(&format!(
                    "SELECT COALESCE(SUM(delta), 0) FROM (
                       SELECT MAX(value)-MIN(value) AS delta
                       FROM snapshot_resource_samples
                       WHERE metric='{metric}' GROUP BY resource_id
                     )"
                ))
                .unwrap_or(0.0)
        })
        .sum()
}

fn max_device_rate(tables: &Tables, metric: &str, duration_units: f64) -> f64 {
    tables
        .scalar_f64(&format!(
            "SELECT COALESCE(MAX(delta), 0) FROM (
               SELECT MAX(value)-MIN(value) AS delta FROM snapshot_resource_samples
               WHERE metric='{metric}' GROUP BY resource_id
             )"
        ))
        .unwrap_or(0.0)
        / duration_units.max(f64::EPSILON)
}

fn max_network_utilization(tables: &Tables, duration_s: f64) -> Result<f64> {
    let mut statement = tables.connection().prepare(
        "SELECT resource_id, metric, MAX(value) AS maximum, MIN(value) AS minimum
         FROM snapshot_resource_samples
         WHERE resource='network' AND metric IN ('receive_bytes','transmit_bytes','link_capacity')
         GROUP BY resource_id, metric",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect::<store::duckdb::Result<Vec<_>>>()?;
    let mut devices = HashMap::<String, (f64, f64)>::new();
    for (device, metric, maximum, minimum) in rows {
        let values = devices.entry(device).or_default();
        if metric == "link_capacity" {
            values.1 = maximum;
        } else {
            values.0 += (maximum - minimum).max(0.0);
        }
    }
    Ok(devices
        .values()
        .filter(|(_, capacity)| *capacity > 0.0)
        .map(|(bytes, capacity)| bytes * 8.0 / duration_s.max(f64::EPSILON) / capacity * 100.0)
        .fold(0.0, f64::max))
}
