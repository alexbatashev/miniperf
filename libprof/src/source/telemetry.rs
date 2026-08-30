//! Host clock and thermal sampling, in any scenario.
//!
//! Frequencies and temperatures are host state, not a feature of one analysis:
//! every recording is conditioned on the clock the part actually ran at. The
//! monitor runs at 1Hz next to the workload and emits the same resource
//! samples the procfs collector does, so a consumer unions the two.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use super::{Availability, SessionContext, Source, SourceDecl};
use crate::{HostTelemetry, HostTelemetrySample, Record, ResourceSample, Sink, SourceStatus};

const INTERVAL: Duration = Duration::from_secs(1);

/// Samples the host's clock and temperature sensors while a workload runs.
#[derive(Default)]
pub struct HostTelemetrySource {
    stop: Option<Arc<AtomicBool>>,
    worker: Option<thread::JoinHandle<Vec<SourceStatus>>>,
}

impl Source for HostTelemetrySource {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn declare(&self) -> SourceDecl {
        SourceDecl {
            name: "host_telemetry",
        }
    }

    fn probe(&self, _directory: &Path) -> Availability {
        Availability::Available
    }

    fn start(&mut self, context: &SessionContext) -> anyhow::Result<()> {
        let clusters = host_clusters();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let sink = context.sink.clone();
        self.worker = Some(
            thread::Builder::new()
                .name("libprof-host-telemetry".to_string())
                .spawn(move || collect(sink.as_ref(), &clusters, worker_stop))?,
        );
        self.stop = Some(stop);
        Ok(())
    }

    fn stop(&mut self, _context: &SessionContext) -> Vec<SourceStatus> {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Release);
        }
        let worker = self.worker.take();
        if let Some(worker) = &worker {
            worker.thread().unpark();
        }
        worker
            .map(|worker| {
                worker.join().unwrap_or_else(|_| {
                    vec![SourceStatus::new(
                        "host_telemetry",
                        "error",
                        "internal",
                        "unavailable",
                        "host telemetry thread did not shut down cleanly",
                    )]
                })
            })
            .unwrap_or_default()
    }
}

/// Cluster id to logical CPUs, from the host's core PMUs. Empty on a
/// homogeneous host, which `HostTelemetry` reads as a single `host` cluster.
fn host_clusters() -> Vec<(String, Vec<u32>)> {
    crate::host_core_clusters()
        .into_iter()
        .map(|cluster| (cluster.family_id, parse_cpumask(&cluster.cpus)))
        .collect()
}

/// Expand a sysfs cpumask such as `"0,5-11"` into its logical CPU numbers.
fn parse_cpumask(mask: &str) -> Vec<u32> {
    mask.split(',')
        .filter_map(|range| {
            let range = range.trim();
            match range.split_once('-') {
                Some((first, last)) => Some(first.parse().ok()?..=last.parse().ok()?),
                None => {
                    let cpu = range.parse().ok()?;
                    Some(cpu..=cpu)
                }
            }
        })
        .flatten()
        .collect()
}

fn collect(
    sink: &dyn Sink,
    clusters: &[(String, Vec<u32>)],
    stop: Arc<AtomicBool>,
) -> Vec<SourceStatus> {
    let mut statuses = Vec::new();
    let mut telemetry = match HostTelemetry::start(clusters) {
        Ok(Some(telemetry)) => telemetry,
        Ok(None) => {
            statuses.push(SourceStatus::new(
                "host_telemetry",
                "unavailable",
                "sysfs",
                "unavailable",
                "the host exposes neither clock nor temperature sensors",
            ));
            return statuses;
        }
        Err(error) => {
            statuses.push(SourceStatus::new(
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
        statuses.push(SourceStatus::new(
            signal,
            "unavailable",
            "sysfs",
            "unavailable",
            reason,
        ));
    }

    let start = Instant::now();
    let mut source = "sysfs";
    loop {
        let timestamp_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        match telemetry.sample() {
            Ok(sample) => {
                if let Some(cluster) = sample.clusters.first() {
                    source = cluster.source;
                }
                for row in telemetry_rows(timestamp_ns, &sample) {
                    sink.record(Record::Resource(row));
                }
            }
            Err(error) => {
                statuses.push(SourceStatus::new(
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
    let discarded = telemetry.discarded_readings();
    statuses.push(if discarded > 0 {
        SourceStatus::new(
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
        SourceStatus::new(
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
fn telemetry_rows(timestamp_ns: u64, sample: &HostTelemetrySample) -> Vec<ResourceSample> {
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
) -> ResourceSample {
    super::resource_sample(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClusterClocks, DeviceClocks, ThermalZone};

    fn find<'a>(rows: &'a [ResourceSample], metric: &str) -> Option<&'a f64> {
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
        assert!(rows
            .iter()
            .any(|row| row.resource == "cpu" && row.metric == "temperature"));
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

    #[test]
    fn expands_sysfs_cpumasks() {
        assert_eq!(parse_cpumask("0,5-8"), vec![0, 5, 6, 7, 8]);
        assert_eq!(parse_cpumask("3"), vec![3]);
        assert!(parse_cpumask("").is_empty());
    }
}
