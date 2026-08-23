//! Host clock and thermal sampling, in any scenario.
//!
//! Frequencies and temperatures are host state, not a snapshot feature: every
//! recording is conditioned on the clock the part actually ran at. The monitor
//! runs at 1Hz next to the workload and writes the same `resource_samples`
//! intermediate the procfs collector writes, so postprocess unions the two.

use mperf_data::{SnapshotCollectorStatus, SnapshotResourceSample};
use pmu::{HostTelemetry, HostTelemetrySample};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct HostTelemetryMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Vec<SnapshotCollectorStatus>>>,
}

impl HostTelemetryMonitor {
    pub(crate) fn start(output: &Path, root_pid: u32) -> std::io::Result<Self> {
        let output = output.to_owned();
        let clusters = host_clusters();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("mperf-host-telemetry".to_string())
            .spawn(move || collect(&output, root_pid, &clusters, worker_stop))?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    pub(crate) fn stop(mut self) -> Vec<SnapshotCollectorStatus> {
        self.stop.store(true, Ordering::Release);
        let worker = self.worker.take();
        if let Some(worker) = &worker {
            worker.thread().unpark();
        }
        worker
            .and_then(|worker| worker.join().ok())
            .unwrap_or_else(|| {
                vec![status(
                    "host_telemetry",
                    "error",
                    "internal",
                    "unavailable",
                    "host telemetry thread did not shut down cleanly",
                )]
            })
    }
}

/// Cluster id to logical CPUs, from the host's core PMUs. Empty on a
/// homogeneous host, which `HostTelemetry` reads as a single `host` cluster.
fn host_clusters() -> Vec<(String, Vec<u32>)> {
    pmu::host_core_clusters()
        .into_iter()
        .map(|cluster| {
            let cpus = crate::postprocess::parse_cpumask(&cluster.cpus)
                .into_iter()
                .flat_map(|(first, last)| first..=last)
                .collect();
            (cluster.family_id, cpus)
        })
        .collect()
}

fn collect(
    output: &Path,
    root_pid: u32,
    clusters: &[(String, Vec<u32>)],
    stop: Arc<AtomicBool>,
) -> Vec<SnapshotCollectorStatus> {
    let mut statuses = Vec::new();
    let mut telemetry = match HostTelemetry::start(clusters) {
        Ok(Some(telemetry)) => telemetry,
        Ok(None) => {
            statuses.push(status(
                "host_telemetry",
                "unavailable",
                "sysfs",
                "unavailable",
                "the host exposes neither clock nor temperature sensors",
            ));
            return statuses;
        }
        Err(error) => {
            statuses.push(status(
                "host_telemetry",
                "unavailable",
                "sysfs",
                "unavailable",
                &error.to_string(),
            ));
            return statuses;
        }
    };
    for (signal, reason) in telemetry.unavailable() {
        statuses.push(status(
            signal,
            "unavailable",
            "sysfs",
            "unavailable",
            reason,
        ));
    }

    // A second writer for the same logical table: the per-process file name
    // keeps its segments from overwriting the procfs collector's.
    let mut writer = store::SegmentWriter::new(
        output,
        "resource_samples",
        Some(root_pid),
        resource_sample_schema(),
    );
    let start = Instant::now();
    let mut source = "sysfs";
    loop {
        let timestamp_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        match telemetry.sample() {
            Ok(sample) => {
                if let Some(cluster) = sample.clusters.first() {
                    source = cluster.source;
                }
                let rows = telemetry_rows(timestamp_ns, &sample);
                let _ = resource_sample_batch(&rows).and_then(|batch| writer.write(&batch));
            }
            Err(error) => {
                statuses.push(status(
                    "host_telemetry",
                    "degraded",
                    source,
                    "best_effort",
                    &error.to_string(),
                ));
                break;
            }
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        thread::park_timeout(INTERVAL);
    }
    let _ = writer.finish();
    let discarded = telemetry.discarded_readings();
    statuses.push(if discarded > 0 {
        status(
            "host_telemetry",
            "degraded",
            source,
            "best_effort",
            &format!(
                "{discarded} clock reading(s) exceeded their cluster ceiling and were \
                 discarded; this host's cpufreq driver reports impossible frequencies"
            ),
        )
    } else {
        status(
            "host_telemetry",
            "available",
            source,
            "exact_system",
            "host clock and temperature sensors",
        )
    });
    statuses
}

/// The rows one tick contributes. Measurements that read as zero carry no
/// information and would drag a mean down, so only ceilings and counters are
/// emitted at zero.
fn telemetry_rows(timestamp_ns: u64, sample: &HostTelemetrySample) -> Vec<SnapshotResourceSample> {
    let mut rows = Vec::new();
    let mut measurement = |resource: &str, id: &str, metric: &str, value: f64, unit, source| {
        if value.is_finite() && value > 0.0 {
            rows.push(host_sample(
                timestamp_ns,
                resource,
                id,
                "utilization",
                metric,
                value,
                unit,
                source,
            ));
        }
    };
    for cluster in &sample.clusters {
        measurement(
            "cpu",
            &cluster.id,
            "frequency",
            cluster.mean_hz,
            "hertz",
            cluster.source,
        );
        measurement(
            "cpu",
            &cluster.id,
            "frequency_peak",
            cluster.peak_hz,
            "hertz",
            cluster.source,
        );
        if let Some(max_hz) = cluster.max_hz {
            measurement(
                "cpu",
                &cluster.id,
                "frequency_max",
                max_hz,
                "hertz",
                cluster.source,
            );
        }
    }
    for device in &sample.devices {
        measurement(
            device.resource,
            &device.id,
            "frequency",
            device.cur_hz,
            "hertz",
            device.source,
        );
        if let Some(max_hz) = device.max_hz {
            measurement(
                device.resource,
                &device.id,
                "frequency_max",
                max_hz,
                "hertz",
                device.source,
            );
        }
        if let Some(busy) = device.busy_percent {
            measurement(
                device.resource,
                &device.id,
                "busy",
                busy,
                "percent",
                device.source,
            );
        }
    }
    for zone in &sample.zones {
        measurement(
            zone.resource,
            &zone.id,
            "temperature",
            zone.celsius,
            "celsius",
            zone.source,
        );
        if let Some(critical) = zone.critical_celsius {
            measurement(
                zone.resource,
                &zone.id,
                "temperature_critical",
                critical,
                "celsius",
                zone.source,
            );
        }
    }
    if let Some(events) = sample.throttle_events {
        rows.push(host_sample(
            timestamp_ns,
            "cpu",
            "host",
            "saturation",
            "throttle_events",
            events as f64,
            "events",
            "sysfs",
        ));
    }
    if let Some(level) = sample.pressure_level {
        rows.push(host_sample(
            timestamp_ns,
            "thermal",
            "host",
            "saturation",
            "thermal_pressure_level",
            level as f64,
            "level",
            "os_thermal_notification",
        ));
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn host_sample(
    timestamp_ns: u64,
    resource: &str,
    id: &str,
    category: &str,
    metric: &str,
    value: f64,
    unit: &str,
    source: &str,
) -> SnapshotResourceSample {
    sample(
        timestamp_ns,
        resource,
        id,
        category,
        metric,
        value,
        unit,
        "system_during_target",
        source,
        "exact_system",
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sample(
    timestamp_ns: u64,
    resource: &str,
    resource_id: &str,
    category: &str,
    metric: &str,
    value: f64,
    unit: &str,
    scope: &str,
    source: &str,
    quality: &str,
) -> SnapshotResourceSample {
    SnapshotResourceSample {
        timestamp_ns,
        resource: resource.to_string(),
        resource_id: resource_id.to_string(),
        category: category.to_string(),
        metric: metric.to_string(),
        value,
        unit: unit.to_string(),
        scope: scope.to_string(),
        source: source.to_string(),
        quality: quality.to_string(),
    }
}

pub(crate) fn status(
    name: &str,
    status_value: &str,
    source: &str,
    quality: &str,
    message: &str,
) -> SnapshotCollectorStatus {
    SnapshotCollectorStatus {
        name: name.to_string(),
        status: status_value.to_string(),
        source: source.to_string(),
        quality: quality.to_string(),
        message: message.to_string(),
    }
}

pub(crate) fn resource_sample_schema() -> Arc<store::arrow::datatypes::Schema> {
    use store::arrow::datatypes::{DataType, Field, Schema};
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ns", DataType::Int64, false),
        Field::new("resource", DataType::Utf8, false),
        Field::new("resource_id", DataType::Utf8, false),
        Field::new("category", DataType::Utf8, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("unit", DataType::Utf8, false),
        Field::new("scope", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("quality", DataType::Utf8, false),
    ]))
}

pub(crate) fn resource_sample_batch(
    samples: &[SnapshotResourceSample],
) -> anyhow::Result<store::arrow::record_batch::RecordBatch> {
    use store::arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
    let text = |field: fn(&SnapshotResourceSample) -> &str| -> ArrayRef {
        Arc::new(StringArray::from(
            samples.iter().map(field).collect::<Vec<_>>(),
        ))
    };
    Ok(store::arrow::record_batch::RecordBatch::try_new(
        resource_sample_schema(),
        vec![
            Arc::new(Int64Array::from(
                samples
                    .iter()
                    .map(|sample| sample.timestamp_ns as i64)
                    .collect::<Vec<_>>(),
            )),
            text(|sample| &sample.resource),
            text(|sample| &sample.resource_id),
            text(|sample| &sample.category),
            text(|sample| &sample.metric),
            Arc::new(Float64Array::from(
                samples
                    .iter()
                    .map(|sample| sample.value)
                    .collect::<Vec<_>>(),
            )),
            text(|sample| &sample.unit),
            text(|sample| &sample.scope),
            text(|sample| &sample.source),
            text(|sample| &sample.quality),
        ],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmu::{ClusterClocks, DeviceClocks, ThermalZone};

    fn find<'a>(rows: &'a [SnapshotResourceSample], metric: &str) -> Option<&'a f64> {
        rows.iter()
            .find(|row| row.metric == metric)
            .map(|row| &row.value)
    }

    #[test]
    fn converts_clocks_and_zones() {
        let rows = telemetry_rows(
            5,
            &HostTelemetrySample {
                clusters: vec![ClusterClocks {
                    id: "cortex_a720".to_string(),
                    mean_hz: 2.1e9,
                    peak_hz: 3.8e9,
                    max_hz: Some(3.8e9),
                    source: "cpufreq",
                }],
                devices: vec![DeviceClocks {
                    id: "gpu0".to_string(),
                    resource: "gpu",
                    cur_hz: 4.09e8,
                    max_hz: Some(1.228e9),
                    busy_percent: Some(37.0),
                    source: "devfreq",
                }],
                zones: vec![ThermalZone {
                    id: "soc".to_string(),
                    resource: "cpu",
                    celsius: 92.4,
                    critical_celsius: Some(94.0),
                    source: "hwmon",
                }],
                throttle_events: Some(0),
                pressure_level: Some(2),
            },
        );
        assert_eq!(find(&rows, "frequency"), Some(&2.1e9));
        assert_eq!(find(&rows, "frequency_peak"), Some(&3.8e9));
        assert_eq!(find(&rows, "frequency_max"), Some(&3.8e9));
        assert_eq!(find(&rows, "temperature"), Some(&92.4));
        let gpu = |metric| {
            rows.iter()
                .find(|row| {
                    row.resource == "gpu" && row.resource_id == "gpu0" && row.metric == metric
                })
                .map(|row| row.value)
        };
        assert_eq!(gpu("frequency"), Some(4.09e8));
        assert_eq!(gpu("frequency_max"), Some(1.228e9));
        assert_eq!(gpu("busy"), Some(37.0));
        assert!(
            rows.iter()
                .any(|row| row.resource == "cpu" && row.metric == "temperature")
        );
        assert_eq!(find(&rows, "temperature_critical"), Some(&94.0));
        assert_eq!(find(&rows, "throttle_events"), Some(&0.0));
        assert_eq!(find(&rows, "thermal_pressure_level"), Some(&2.0));
        assert!(rows.iter().all(|row| row.timestamp_ns == 5));
        assert!(rows.iter().all(|row| row.quality == "exact_system"));
    }

    #[test]
    fn skips_unmeasured_clocks_and_temperatures() {
        let rows = telemetry_rows(
            0,
            &HostTelemetrySample {
                clusters: vec![ClusterClocks {
                    id: "host".to_string(),
                    mean_hz: 0.0,
                    peak_hz: f64::NAN,
                    max_hz: Some(4.0e9),
                    source: "pmu_derived",
                }],
                devices: vec![],
                zones: vec![ThermalZone {
                    id: "soc".to_string(),
                    resource: "thermal",
                    celsius: 0.0,
                    critical_celsius: None,
                    source: "hwmon",
                }],
                throttle_events: None,
                pressure_level: None,
            },
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.metric.as_str())
                .collect::<Vec<_>>(),
            vec!["frequency_max"]
        );
    }
}
