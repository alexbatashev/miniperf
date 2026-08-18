//! Lightweight Linux resource telemetry for the snapshot scenario.

use mperf_data::{SnapshotCollectorStatus, SnapshotProcessInfo, SnapshotResourceSample};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct ResourceMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Vec<SnapshotCollectorStatus>>>,
}

pub(crate) struct CgroupScope {
    path: Option<PathBuf>,
    status: SnapshotCollectorStatus,
}

impl CgroupScope {
    pub(crate) fn create(root_pid: u32, launched: bool) -> Self {
        if !launched {
            return Self {
                path: None,
                status: status(
                    "cgroup",
                    "unavailable",
                    "cgroup_v2",
                    "best_effort",
                    "attached processes are not moved; using non-invasive procfs/BPF tracking",
                ),
            };
        }
        let relative = fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|value| {
                value
                    .lines()
                    .find_map(|line| line.strip_prefix("0::").map(str::to_string))
            });
        let Some(relative) = relative else {
            return Self::failed("unified cgroup v2 is unavailable");
        };
        let parent = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        let path = parent.join(format!("mperf-snapshot-{root_pid}"));
        if let Err(error) = fs::create_dir(&path) {
            return Self::failed(&format!("cgroup delegation is unavailable: {error}"));
        }
        if let Err(error) = fs::write(path.join("cgroup.procs"), root_pid.to_string()) {
            let _ = fs::remove_dir(&path);
            return Self::failed(&format!(
                "could not place target in private cgroup: {error}"
            ));
        }
        Self {
            path: Some(path),
            status: status(
                "cgroup",
                "available",
                "cgroup_v2",
                "exact_process_tree",
                "launched descendants inherit a private cgroup",
            ),
        }
    }

    fn failed(message: &str) -> Self {
        Self {
            path: None,
            status: status("cgroup", "unavailable", "cgroup_v2", "best_effort", message),
        }
    }

    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.path.clone()
    }

    pub(crate) fn status(&self) -> SnapshotCollectorStatus {
        self.status.clone()
    }
}

impl Drop for CgroupScope {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_dir(path);
        }
    }
}

/// Optional BPF tracepoint collector. It deliberately has no privilege helper:
/// callers either already have BPF access or receive a degraded status.
pub(crate) struct BpfMonitor {
    child: Option<std::process::Child>,
    output: PathBuf,
    status: SnapshotCollectorStatus,
}

impl BpfMonitor {
    pub(crate) fn start(root_pid: u32, output_directory: &Path) -> Self {
        let output = output_directory.join("snapshot-bpf.txt");
        if !Path::new("/sys/kernel/btf/vmlinux").is_file() {
            return Self::unavailable(output, "kernel BTF is unavailable");
        }
        let disabled = fs::read_to_string("/proc/sys/kernel/unprivileged_bpf_disabled")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if unsafe { libc::geteuid() } != 0 && disabled != 0 {
            return Self::unavailable(
                output,
                "unprivileged BPF is disabled; run with suitable BPF/perf capabilities",
            );
        }
        let script = format!(
            r#"
BEGIN {{ @tracked[{root_pid}] = 1; printf("MPERF_READY\n"); }}
tracepoint:sched:sched_process_fork /@tracked[args->parent_pid]/ {{ @tracked[args->child_pid] = 1; }}
tracepoint:sched:sched_wakeup /@tracked[args->pid]/ {{ @wakeup[args->pid] = nsecs; }}
tracepoint:sched:sched_wakeup_new /@tracked[args->pid]/ {{ @wakeup[args->pid] = nsecs; }}
tracepoint:sched:sched_switch /@tracked[args->next_pid] && @wakeup[args->next_pid]/ {{ @runq_ns = sum(nsecs - @wakeup[args->next_pid]); @runq_count = count(); delete(@wakeup[args->next_pid]); }}
tracepoint:block:block_rq_issue /@tracked[pid]/ {{ @request[args->dev, args->sector] = nsecs; @block_bytes = sum(args->bytes); }}
tracepoint:block:block_rq_complete /@request[args->dev, args->sector]/ {{ @block_latency_ns = sum(nsecs - @request[args->dev, args->sector]); @block_count = count(); delete(@request[args->dev, args->sector]); }}
tracepoint:tcp:tcp_retransmit_skb /@tracked[pid]/ {{ @tcp_retransmits = count(); }}
END {{ printf("MPERF runq_ns %llu\n", @runq_ns); printf("MPERF runq_count %llu\n", @runq_count); printf("MPERF block_bytes %llu\n", @block_bytes); printf("MPERF block_latency_ns %llu\n", @block_latency_ns); printf("MPERF block_count %llu\n", @block_count); printf("MPERF tcp_retransmits %llu\n", @tcp_retransmits); }}
"#
        );
        let child = std::process::Command::new("bpftrace")
            .args(["-q", "-B", "line", "-o"])
            .arg(&output)
            .args(["-e", script.as_str()])
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline {
                    if fs::read_to_string(&output).is_ok_and(|value| value.contains("MPERF_READY"))
                    {
                        return Self {
                            child: Some(child),
                            output,
                            status: status(
                                "bpf",
                                "available",
                                "bpftrace/tracepoints",
                                "process_tree",
                                "scheduler, block, and TCP tracepoints",
                            ),
                        };
                    }
                    if child.try_wait().ok().flatten().is_some() {
                        return Self::unavailable(
                            output,
                            "BPF program failed to load; inspect kernel capabilities and tracepoint availability",
                        );
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
                let _ = child.wait();
                Self::unavailable(output, "timed out while loading BPF tracepoints")
            }
            Err(error) => Self::unavailable(output, &format!("could not start bpftrace: {error}")),
        }
    }

    fn unavailable(output: PathBuf, message: &str) -> Self {
        Self {
            child: None,
            output,
            status: status(
                "bpf",
                if message.contains("disabled") {
                    "permission_denied"
                } else {
                    "unavailable"
                },
                "bpftrace/tracepoints",
                "unavailable",
                message,
            ),
        }
    }

    pub(crate) fn stop(mut self) -> SnapshotCollectorStatus {
        if let Some(mut child) = self.child.take() {
            unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
            if child.wait().is_err() {
                self.status.status = "error".to_string();
                self.status.quality = "unavailable".to_string();
                self.status.message = "failed to stop BPF collector cleanly".to_string();
            }
            if self.write_metrics_table().is_err() {
                self.status.status = "error".to_string();
                self.status.message = "failed to persist BPF metrics".to_string();
            }
        }
        self.status
    }

    /// Convert bpftrace's final `MPERF <metric> <value>` lines into the
    /// `bpf_metrics` Parquet table and drop the raw text.
    fn write_metrics_table(&self) -> anyhow::Result<()> {
        use store::arrow::array::{ArrayRef, Float64Array, StringArray};
        use store::arrow::datatypes::{DataType, Field, Schema};

        let text = fs::read_to_string(&self.output)?;
        let mut metrics = Vec::new();
        let mut values = Vec::new();
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            if fields.next() != Some("MPERF") {
                continue;
            }
            let (Some(metric), Some(value)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Ok(value) = value.parse::<f64>() else {
                continue;
            };
            metrics.push(metric.to_string());
            values.push(value);
        }
        let directory = self
            .output
            .parent()
            .ok_or_else(|| anyhow::anyhow!("BPF output has no parent directory"))?;
        if !metrics.is_empty() {
            let schema = std::sync::Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
            ]));
            let batch = store::arrow::record_batch::RecordBatch::try_new(
                schema.clone(),
                vec![
                    std::sync::Arc::new(StringArray::from(metrics)) as ArrayRef,
                    std::sync::Arc::new(Float64Array::from(values)),
                ],
            )?;
            let mut writer = store::SegmentWriter::new(directory, "bpf_metrics", None, schema);
            writer.write(&batch)?;
            writer.finish()?;
        }
        fs::remove_file(&self.output)?;
        Ok(())
    }
}

impl ResourceMonitor {
    pub(crate) fn start(
        root_pid: u32,
        launched: bool,
        cgroup: Option<PathBuf>,
        output: &Path,
    ) -> std::io::Result<Self> {
        let output = output.to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("mperf-snapshot-resources".to_string())
            .spawn(move || collect(root_pid, launched, cgroup, &output, worker_stop))?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    pub(crate) fn stop(mut self) -> Vec<SnapshotCollectorStatus> {
        self.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .and_then(|worker| worker.join().ok())
            .unwrap_or_else(|| {
                vec![status(
                    "resource_monitor",
                    "error",
                    "internal",
                    "unavailable",
                    "resource collector thread did not shut down cleanly",
                )]
            })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcStat {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) state: u8,
    pub(crate) command: String,
    pub(crate) start_ticks: u64,
    user_ticks: u64,
    system_ticks: u64,
    minor_faults: u64,
    major_faults: u64,
    rss_pages: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcIo {
    read_bytes: u64,
    write_bytes: u64,
    read_calls: u64,
    write_calls: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcContext {
    voluntary: u64,
    involuntary: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcTotals {
    user_ticks: u64,
    system_ticks: u64,
    minor_faults: u64,
    major_faults: u64,
    io: ProcIo,
    context: ProcContext,
}

pub(crate) fn process_tree(root_pid: u32) -> Vec<ProcStat> {
    let mut all = HashMap::<u32, ProcStat>::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if let Some(stat) = read_proc_stat(pid) {
            all.insert(pid, stat);
        }
    }
    if !all.contains_key(&root_pid) {
        return Vec::new();
    }
    let mut children = HashMap::<u32, Vec<u32>>::new();
    for stat in all.values() {
        children.entry(stat.ppid).or_default().push(stat.pid);
    }
    let mut result = Vec::new();
    let mut queue = VecDeque::from([root_pid]);
    let mut seen = HashSet::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(stat) = all.get(&pid) {
            result.push(stat.clone());
            queue.extend(children.get(&pid).into_iter().flatten().copied());
        }
    }
    result
}

fn collect(
    root_pid: u32,
    launched: bool,
    cgroup: Option<PathBuf>,
    output: &Path,
    stop: Arc<AtomicBool>,
) -> Vec<SnapshotCollectorStatus> {
    let start = Instant::now();
    // Roll small segments so a crash loses at most the open one.
    let mut writer =
        store::SegmentWriter::new(output, "resource_samples", None, resource_sample_schema())
            .with_segment_bytes(256 * 1024);
    let mut previous = HashMap::<(u32, u64), ProcTotals>::new();
    let mut cumulative = ProcTotals::default();
    let mut processes = HashMap::<(u32, u64), SnapshotProcessInfo>::new();
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as f64;
    let mut statuses = capability_statuses();
    let mut initial_scan = true;
    let mut memory_controller = match pmu::MemoryControllerMonitor::start() {
        Ok(Some(monitor)) => {
            statuses.push(status(
                "uncore_memory",
                "available",
                monitor.source(),
                "system_during_target",
                "memory-controller read/write counters",
            ));
            Some(monitor)
        }
        Ok(None) => {
            statuses.push(status(
                "uncore_memory",
                "unavailable",
                "perf_event/sysfs",
                "unavailable",
                "no memory-controller PMU aliases and no vendor bandwidth device",
            ));
            None
        }
        Err(error) => {
            statuses.push(status(
                "uncore_memory",
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    "permission_denied"
                } else {
                    "error"
                },
                "perf_event/sysfs",
                "unavailable",
                &error.to_string(),
            ));
            None
        }
    };

    loop {
        let timestamp_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let tree = process_tree(root_pid);
        let mut rss_bytes = 0_f64;
        let mut pss_bytes = 0_f64;
        let mut current = HashMap::new();
        for stat in tree {
            let key = (stat.pid, stat.start_ticks);
            let totals = ProcTotals {
                user_ticks: stat.user_ticks,
                system_ticks: stat.system_ticks,
                minor_faults: stat.minor_faults,
                major_faults: stat.major_faults,
                io: read_proc_io(stat.pid),
                context: read_proc_context(stat.pid),
            };
            if let Some(before) = previous.get(&key) {
                cumulative.user_ticks = cumulative
                    .user_ticks
                    .saturating_add(totals.user_ticks.saturating_sub(before.user_ticks));
                cumulative.system_ticks = cumulative
                    .system_ticks
                    .saturating_add(totals.system_ticks.saturating_sub(before.system_ticks));
                cumulative.minor_faults = cumulative
                    .minor_faults
                    .saturating_add(totals.minor_faults.saturating_sub(before.minor_faults));
                cumulative.major_faults = cumulative
                    .major_faults
                    .saturating_add(totals.major_faults.saturating_sub(before.major_faults));
                add_io_delta(&mut cumulative.io, totals.io, before.io);
                cumulative.context.voluntary = cumulative.context.voluntary.saturating_add(
                    totals
                        .context
                        .voluntary
                        .saturating_sub(before.context.voluntary),
                );
                cumulative.context.involuntary = cumulative.context.involuntary.saturating_add(
                    totals
                        .context
                        .involuntary
                        .saturating_sub(before.context.involuntary),
                );
            } else if launched || !initial_scan {
                cumulative.user_ticks = cumulative.user_ticks.saturating_add(totals.user_ticks);
                cumulative.system_ticks =
                    cumulative.system_ticks.saturating_add(totals.system_ticks);
                cumulative.minor_faults =
                    cumulative.minor_faults.saturating_add(totals.minor_faults);
                cumulative.major_faults =
                    cumulative.major_faults.saturating_add(totals.major_faults);
                cumulative.io.read_bytes = cumulative
                    .io
                    .read_bytes
                    .saturating_add(totals.io.read_bytes);
                cumulative.io.write_bytes = cumulative
                    .io
                    .write_bytes
                    .saturating_add(totals.io.write_bytes);
                cumulative.io.read_calls = cumulative
                    .io
                    .read_calls
                    .saturating_add(totals.io.read_calls);
                cumulative.io.write_calls = cumulative
                    .io
                    .write_calls
                    .saturating_add(totals.io.write_calls);
                cumulative.context.voluntary = cumulative
                    .context
                    .voluntary
                    .saturating_add(totals.context.voluntary);
                cumulative.context.involuntary = cumulative
                    .context
                    .involuntary
                    .saturating_add(totals.context.involuntary);
            }
            rss_bytes += stat.rss_pages.max(0) as f64 * page_size;
            pss_bytes += key_values(&format!("/proc/{}/smaps_rollup", stat.pid))
                .get("Pss")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0)
                * 1024.0;
            let process = processes.entry(key).or_insert_with(|| SnapshotProcessInfo {
                pid: stat.pid,
                ppid: stat.ppid,
                start_ticks: stat.start_ticks,
                first_seen_ns: timestamp_ns,
                last_seen_ns: timestamp_ns,
                command: stat.command.clone(),
                quality: "procfs_best_effort".to_string(),
            });
            process.last_seen_ns = timestamp_ns;
            current.insert(key, totals);
        }
        previous = current;
        initial_scan = false;

        let mut samples = vec![
            tree_sample(
                timestamp_ns,
                "cpu",
                "utilization",
                "user_time",
                cumulative.user_ticks as f64 / ticks_per_second,
                "seconds",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "cpu",
                "utilization",
                "system_time",
                cumulative.system_ticks as f64 / ticks_per_second,
                "seconds",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "cpu",
                "errors",
                "minor_faults",
                cumulative.minor_faults as f64,
                "faults",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "memory",
                "errors",
                "major_faults",
                cumulative.major_faults as f64,
                "faults",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "memory",
                "utilization",
                "pss",
                pss_bytes,
                "bytes",
                "procfs_smaps_rollup",
            ),
            tree_sample(
                timestamp_ns,
                "memory",
                "utilization",
                "rss",
                rss_bytes,
                "bytes",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "disk",
                "utilization",
                "read_bytes",
                cumulative.io.read_bytes as f64,
                "bytes",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "disk",
                "utilization",
                "write_bytes",
                cumulative.io.write_bytes as f64,
                "bytes",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "disk",
                "utilization",
                "read_calls",
                cumulative.io.read_calls as f64,
                "operations",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "disk",
                "utilization",
                "write_calls",
                cumulative.io.write_calls as f64,
                "operations",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "cpu",
                "saturation",
                "voluntary_context_switches",
                cumulative.context.voluntary as f64,
                "switches",
                "procfs",
            ),
            tree_sample(
                timestamp_ns,
                "cpu",
                "saturation",
                "involuntary_context_switches",
                cumulative.context.involuntary as f64,
                "switches",
                "procfs",
            ),
        ];
        collect_pressure(timestamp_ns, &mut samples);
        collect_meminfo(timestamp_ns, &mut samples);
        collect_diskstats(timestamp_ns, &mut samples);
        collect_network(timestamp_ns, &mut samples);
        if let Some(cgroup) = &cgroup {
            collect_cgroup(timestamp_ns, cgroup, &mut samples);
        }
        if let Some(monitor) = memory_controller.as_mut() {
            match monitor.sample() {
                Ok(sample) => {
                    samples.push(system_sample(
                        timestamp_ns,
                        "memory",
                        "utilization",
                        "dram_read_bytes",
                        sample.read_bytes as f64,
                        "bytes",
                        "uncore_pmu",
                    ));
                    samples.push(system_sample(
                        timestamp_ns,
                        "memory",
                        "utilization",
                        "dram_write_bytes",
                        sample.write_bytes as f64,
                        "bytes",
                        "uncore_pmu",
                    ));
                    if let Some(joules) = sample.package_joules {
                        samples.push(system_sample(
                            timestamp_ns,
                            "cpu",
                            "utilization",
                            "package_energy",
                            joules,
                            "joules",
                            "power_pmu",
                        ));
                    }
                    if let Some(joules) = sample.core_joules {
                        samples.push(system_sample(
                            timestamp_ns,
                            "cpu",
                            "utilization",
                            "core_energy",
                            joules,
                            "joules",
                            "power_pmu",
                        ));
                    }
                }
                Err(error) => {
                    statuses.push(status(
                        "uncore_memory",
                        "error",
                        "perf_event/sysfs",
                        "unavailable",
                        &error.to_string(),
                    ));
                    memory_controller = None;
                }
            }
        }
        let written = resource_sample_batch(&samples).and_then(|batch| writer.write(&batch));
        if written.is_err() {
            statuses.push(status(
                "resource_file",
                "error",
                "parquet",
                "unavailable",
                "failed to write resource samples",
            ));
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        thread::park_timeout(INTERVAL);
    }
    let _ = writer.finish();

    let mut rows = processes.into_values().collect::<Vec<_>>();
    rows.sort_by_key(|process| (process.first_seen_ns, process.pid));
    let written = process_batch(&rows).and_then(|batch| {
        let mut writer =
            store::SegmentWriter::new(output, "process_samples", None, process_schema());
        writer.write(&batch)?;
        writer.finish().map(|_| ())
    });
    if written.is_err() {
        statuses.push(status(
            "resource_file",
            "error",
            "parquet",
            "unavailable",
            "failed to write the observed process tree",
        ));
    }
    statuses
}

fn resource_sample_schema() -> std::sync::Arc<store::arrow::datatypes::Schema> {
    use store::arrow::datatypes::{DataType, Field, Schema};
    std::sync::Arc::new(Schema::new(vec![
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

fn resource_sample_batch(
    samples: &[SnapshotResourceSample],
) -> anyhow::Result<store::arrow::record_batch::RecordBatch> {
    use store::arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
    let text = |field: fn(&SnapshotResourceSample) -> &str| -> ArrayRef {
        std::sync::Arc::new(StringArray::from(
            samples.iter().map(field).collect::<Vec<_>>(),
        ))
    };
    Ok(store::arrow::record_batch::RecordBatch::try_new(
        resource_sample_schema(),
        vec![
            std::sync::Arc::new(Int64Array::from(
                samples
                    .iter()
                    .map(|sample| sample.timestamp_ns as i64)
                    .collect::<Vec<_>>(),
            )),
            text(|sample| &sample.resource),
            text(|sample| &sample.resource_id),
            text(|sample| &sample.category),
            text(|sample| &sample.metric),
            std::sync::Arc::new(Float64Array::from(
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

fn process_schema() -> std::sync::Arc<store::arrow::datatypes::Schema> {
    use store::arrow::datatypes::{DataType, Field, Schema};
    std::sync::Arc::new(Schema::new(vec![
        Field::new("pid", DataType::Int64, false),
        Field::new("ppid", DataType::Int64, false),
        Field::new("start_ticks", DataType::Int64, false),
        Field::new("first_seen_ns", DataType::Int64, false),
        Field::new("last_seen_ns", DataType::Int64, false),
        Field::new("command", DataType::Utf8, false),
        Field::new("quality", DataType::Utf8, false),
    ]))
}

fn process_batch(
    rows: &[mperf_data::SnapshotProcessInfo],
) -> anyhow::Result<store::arrow::record_batch::RecordBatch> {
    use store::arrow::array::{ArrayRef, Int64Array, StringArray};
    let int = |field: fn(&mperf_data::SnapshotProcessInfo) -> i64| -> ArrayRef {
        std::sync::Arc::new(Int64Array::from(rows.iter().map(field).collect::<Vec<_>>()))
    };
    Ok(store::arrow::record_batch::RecordBatch::try_new(
        process_schema(),
        vec![
            int(|row| row.pid as i64),
            int(|row| row.ppid as i64),
            int(|row| row.start_ticks as i64),
            int(|row| row.first_seen_ns as i64),
            int(|row| row.last_seen_ns as i64),
            std::sync::Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.command.as_str())
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.quality.as_str())
                    .collect::<Vec<_>>(),
            )),
        ],
    )?)
}

fn capability_statuses() -> Vec<SnapshotCollectorStatus> {
    vec![status(
        "process_tree",
        "available",
        "procfs",
        "best_effort",
        "existing and future descendants are polled by PID/start-time identity",
    )]
}

fn read_proc_stat(pid: u32) -> Option<ProcStat> {
    let value = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    let command = value[open + 1..close].to_string();
    let fields = value[close + 2..].split_whitespace().collect::<Vec<_>>();
    Some(ProcStat {
        pid,
        state: fields.first()?.as_bytes().first().copied()?,
        ppid: fields.get(1)?.parse().ok()?,
        minor_faults: fields.get(7)?.parse().ok()?,
        major_faults: fields.get(9)?.parse().ok()?,
        user_ticks: fields.get(11)?.parse().ok()?,
        system_ticks: fields.get(12)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
        rss_pages: fields.get(21)?.parse().ok()?,
        command,
    })
}

fn read_proc_io(pid: u32) -> ProcIo {
    let values = key_values(&format!("/proc/{pid}/io"));
    ProcIo {
        read_bytes: get_u64(&values, "read_bytes"),
        write_bytes: get_u64(&values, "write_bytes"),
        read_calls: get_u64(&values, "syscr"),
        write_calls: get_u64(&values, "syscw"),
    }
}

fn read_proc_context(pid: u32) -> ProcContext {
    let values = key_values(&format!("/proc/{pid}/status"));
    ProcContext {
        voluntary: get_u64(&values, "voluntary_ctxt_switches"),
        involuntary: get_u64(&values, "nonvoluntary_ctxt_switches"),
    }
}

fn key_values(path: &str) -> HashMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_string(), value.trim().to_string()))
        .collect()
}

fn whitespace_values(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .map(|(key, value)| (key.to_string(), value.trim().to_string()))
        .collect()
}

fn get_u64(values: &HashMap<String, String>, key: &str) -> u64 {
    values
        .get(key)
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn add_io_delta(total: &mut ProcIo, after: ProcIo, before: ProcIo) {
    total.read_bytes = total
        .read_bytes
        .saturating_add(after.read_bytes.saturating_sub(before.read_bytes));
    total.write_bytes = total
        .write_bytes
        .saturating_add(after.write_bytes.saturating_sub(before.write_bytes));
    total.read_calls = total
        .read_calls
        .saturating_add(after.read_calls.saturating_sub(before.read_calls));
    total.write_calls = total
        .write_calls
        .saturating_add(after.write_calls.saturating_sub(before.write_calls));
}

fn collect_pressure(timestamp_ns: u64, samples: &mut Vec<SnapshotResourceSample>) {
    for resource in ["cpu", "memory", "io"] {
        let path = format!("/proc/pressure/{resource}");
        let Ok(value) = fs::read_to_string(path) else {
            continue;
        };
        for line in value.lines() {
            let mut fields = line.split_whitespace();
            let Some(kind) = fields.next() else { continue };
            for field in fields {
                let Some((name, value)) = field.split_once('=') else {
                    continue;
                };
                if name != "avg10" {
                    continue;
                }
                if let Ok(value) = value.parse::<f64>() {
                    samples.push(system_sample(
                        timestamp_ns,
                        resource,
                        "saturation",
                        &format!("psi_{kind}_avg10"),
                        value,
                        "percent",
                        "procfs_psi",
                    ));
                }
            }
        }
    }
}

fn collect_meminfo(timestamp_ns: u64, samples: &mut Vec<SnapshotResourceSample>) {
    let values = key_values("/proc/meminfo");
    for (key, metric) in [
        ("MemTotal", "host_total"),
        ("MemAvailable", "host_available"),
        ("SwapTotal", "swap_total"),
        ("SwapFree", "swap_free"),
    ] {
        if let Some(kib) = values
            .get(key)
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok())
        {
            samples.push(system_sample(
                timestamp_ns,
                "memory",
                "utilization",
                metric,
                kib * 1024.0,
                "bytes",
                "procfs",
            ));
        }
    }
}

fn collect_diskstats(timestamp_ns: u64, samples: &mut Vec<SnapshotResourceSample>) {
    let Ok(value) = fs::read_to_string("/proc/diskstats") else {
        return;
    };
    for line in value.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 14 {
            continue;
        }
        let device = fields[2];
        let Some(io_ms) = fields[12].parse::<f64>().ok() else {
            continue;
        };
        let Some(weighted_ms) = fields[13].parse::<f64>().ok() else {
            continue;
        };
        samples.push(device_sample(
            timestamp_ns,
            "disk",
            device,
            "utilization",
            "busy_time",
            io_ms,
            "milliseconds",
            "procfs_diskstats",
        ));
        samples.push(device_sample(
            timestamp_ns,
            "disk",
            device,
            "saturation",
            "weighted_io_time",
            weighted_ms,
            "milliseconds",
            "procfs_diskstats",
        ));
    }
}

fn collect_network(timestamp_ns: u64, samples: &mut Vec<SnapshotResourceSample>) {
    let Ok(value) = fs::read_to_string("/proc/net/dev") else {
        return;
    };
    for line in value.lines().skip(2) {
        let Some((interface, rest)) = line.split_once(':') else {
            continue;
        };
        let fields = rest.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 16 {
            continue;
        }
        for (metric, index, category) in [
            ("receive_bytes", 0, "utilization"),
            ("receive_errors", 2, "errors"),
            ("receive_drops", 3, "errors"),
            ("transmit_bytes", 8, "utilization"),
            ("transmit_errors", 10, "errors"),
            ("transmit_drops", 11, "errors"),
        ] {
            if let Ok(value) = fields[index].parse::<f64>() {
                samples.push(device_sample(
                    timestamp_ns,
                    "network",
                    interface.trim(),
                    category,
                    metric,
                    value,
                    if metric.ends_with("bytes") {
                        "bytes"
                    } else {
                        "events"
                    },
                    "procfs_netdev",
                ));
            }
        }
        if let Some(mbps) = fs::read_to_string(format!("/sys/class/net/{}/speed", interface.trim()))
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| *value > 0.0)
        {
            samples.push(device_sample(
                timestamp_ns,
                "network",
                interface.trim(),
                "utilization",
                "link_capacity",
                mbps * 1_000_000.0,
                "bits_per_second",
                "sysfs",
            ));
        }
    }
}

fn collect_cgroup(timestamp_ns: u64, path: &Path, samples: &mut Vec<SnapshotResourceSample>) {
    let push = |samples: &mut Vec<SnapshotResourceSample>,
                resource: &str,
                category: &str,
                metric: &str,
                value: f64,
                unit: &str| {
        samples.push(sample(
            timestamp_ns,
            resource,
            "private_cgroup",
            category,
            metric,
            value,
            unit,
            "process_tree",
            "cgroup_v2",
            "exact_process_tree",
        ));
    };
    let cpu = whitespace_values(&path.join("cpu.stat"));
    for (key, metric, unit, scale, category) in [
        (
            "usage_usec",
            "cgroup_cpu_time",
            "seconds",
            1e-6,
            "utilization",
        ),
        (
            "user_usec",
            "cgroup_user_time",
            "seconds",
            1e-6,
            "utilization",
        ),
        (
            "system_usec",
            "cgroup_system_time",
            "seconds",
            1e-6,
            "utilization",
        ),
        (
            "nr_throttled",
            "cpu_throttle_events",
            "events",
            1.0,
            "saturation",
        ),
        (
            "throttled_usec",
            "cpu_throttled_time",
            "seconds",
            1e-6,
            "saturation",
        ),
    ] {
        if let Some(value) = cpu.get(key).and_then(|value| value.parse::<f64>().ok()) {
            push(samples, "cpu", category, metric, value * scale, unit);
        }
    }
    for (file, metric) in [
        ("memory.current", "cgroup_memory_current"),
        ("memory.peak", "cgroup_memory_peak"),
        ("memory.max", "cgroup_memory_limit"),
    ] {
        if let Some(value) = fs::read_to_string(path.join(file))
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
        {
            push(samples, "memory", "utilization", metric, value, "bytes");
        }
    }
    let memory_events = whitespace_values(&path.join("memory.events"));
    for (key, metric) in [
        ("oom", "oom_events"),
        ("oom_kill", "oom_kills"),
        ("max", "memory_limit_events"),
    ] {
        if let Some(value) = memory_events
            .get(key)
            .and_then(|value| value.parse::<f64>().ok())
        {
            push(samples, "memory", "errors", metric, value, "events");
        }
    }
    if let Ok(io) = fs::read_to_string(path.join("io.stat")) {
        for line in io.lines() {
            let mut fields = line.split_whitespace();
            let device = fields.next().unwrap_or("unknown");
            for field in fields {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                let Ok(value) = value.parse::<f64>() else {
                    continue;
                };
                let (metric, unit) = match key {
                    "rbytes" => ("cgroup_read_bytes", "bytes"),
                    "wbytes" => ("cgroup_write_bytes", "bytes"),
                    "rios" => ("cgroup_read_operations", "operations"),
                    "wios" => ("cgroup_write_operations", "operations"),
                    _ => continue,
                };
                samples.push(sample(
                    timestamp_ns,
                    "disk",
                    device,
                    "utilization",
                    metric,
                    value,
                    unit,
                    "process_tree",
                    "cgroup_v2",
                    "exact_process_tree",
                ));
            }
        }
    }
    for resource in ["cpu", "memory", "io"] {
        let Ok(value) = fs::read_to_string(path.join(format!("{resource}.pressure"))) else {
            continue;
        };
        for line in value.lines() {
            let mut fields = line.split_whitespace();
            let Some(kind) = fields.next() else { continue };
            for field in fields {
                let Some(("avg10", value)) = field.split_once('=') else {
                    continue;
                };
                if let Ok(value) = value.parse::<f64>() {
                    push(
                        samples,
                        resource,
                        "saturation",
                        &format!("cgroup_psi_{kind}_avg10"),
                        value,
                        "percent",
                    );
                }
            }
        }
    }
}

fn tree_sample(
    timestamp_ns: u64,
    resource: &str,
    category: &str,
    metric: &str,
    value: f64,
    unit: &str,
    source: &str,
) -> SnapshotResourceSample {
    sample(
        timestamp_ns,
        resource,
        "process_tree",
        category,
        metric,
        value,
        unit,
        "process_tree",
        source,
        "best_effort",
    )
}

fn system_sample(
    timestamp_ns: u64,
    resource: &str,
    category: &str,
    metric: &str,
    value: f64,
    unit: &str,
    source: &str,
) -> SnapshotResourceSample {
    sample(
        timestamp_ns,
        resource,
        "host",
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
fn device_sample(
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
fn sample(
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

fn status(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_in_its_own_tree() {
        let tree = process_tree(std::process::id());
        assert!(tree.iter().any(|process| process.pid == std::process::id()));
    }

    #[test]
    fn proc_stat_parser_reads_current_identity() {
        let stat = read_proc_stat(std::process::id()).unwrap();
        assert_eq!(stat.pid, std::process::id());
        assert!(stat.start_ticks > 0);
        assert!(!stat.command.is_empty());
    }
}
