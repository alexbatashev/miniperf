use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use mperf_data::{EventType, ProcMapEntry, RecordInfo, ScenarioInfo};
use object::{Object, ObjectSegment};
use serde::Deserialize;
use store::EventKind;

use super::tables::{Columns, Tables};
use crate::utils;

#[derive(Default)]
struct RooflineLoopInfo {
    id: u64,
    pid: u32,
    tid: u32,
    file_name: u64,
    func_name: u64,
    line: u32,
    start: i64,
    bytes_load: i64,
    bytes_store: i64,
    scalar_int_ops: i64,
    scalar_float_ops: i64,
    scalar_double_ops: i64,
    vector_int_ops: i64,
    vector_float_ops: i64,
    vector_double_ops: i64,
}

struct Payload {
    function_id: u64,
    file_id: u64,
    line: u32,
}

/// One row of the recorded `events` table.
struct TraceEvent {
    timestamp: i64,
    event_id: u64,
    instance: u64,
    parent_id: u64,
    flow_id: u64,
    kind: u8,
    pid: u32,
    tid: u32,
    value: i64,
}

struct RooflineData {
    baseline_pid: i32,
    instrumented_pid: i32,
    loops: HashMap<u64, RooflineLoopInfo>,
    runs: Vec<(RooflineLoopInfo, i64)>,
    ops: Vec<RooflineLoopInfo>,
}

impl RooflineData {
    fn consume(
        &mut self,
        event: &TraceEvent,
        payloads: &HashMap<u64, Payload>,
        names: &HashMap<u64, String>,
    ) -> Result<()> {
        if event.kind == EventKind::Begin as u8 {
            let payload = payloads.get(&event.event_id).ok_or_else(|| {
                anyhow::anyhow!("roofline loop start has no payload {}", event.event_id)
            })?;
            self.loops.insert(
                event.instance,
                RooflineLoopInfo {
                    id: event.instance,
                    pid: event.pid,
                    tid: event.tid,
                    file_name: payload.file_id,
                    func_name: payload.function_id,
                    line: payload.line,
                    start: event.timestamp,
                    ..RooflineLoopInfo::default()
                },
            );
            return Ok(());
        }
        if event.kind == EventKind::End as u8 {
            let loop_info = self.loops.remove(&event.flow_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "roofline loop end references unknown loop {}",
                    event.flow_id
                )
            })?;
            if event.pid as i32 == self.baseline_pid {
                self.runs.push((loop_info, event.timestamp));
            } else if event.pid as i32 == self.instrumented_pid {
                self.ops.push(loop_info);
            }
            return Ok(());
        }
        if event.kind != EventKind::Counter as u8 {
            return Ok(());
        }
        let name = names.get(&event.event_id).map(String::as_str).unwrap_or("");
        let Some(ty) = utils::event_type_from_name(name) else {
            return Ok(());
        };
        let loop_info = self.loops.get_mut(&event.parent_id).ok_or_else(|| {
            anyhow::anyhow!(
                "roofline event references unknown parent {}",
                event.parent_id
            )
        })?;
        match ty {
            EventType::RooflineBytesLoad => loop_info.bytes_load = event.value,
            EventType::RooflineBytesStore => loop_info.bytes_store = event.value,
            EventType::RooflineScalarIntOps => loop_info.scalar_int_ops = event.value,
            EventType::RooflineScalarFloatOps => loop_info.scalar_float_ops = event.value,
            EventType::RooflineScalarDoubleOps => loop_info.scalar_double_ops = event.value,
            EventType::RooflineVectorIntOps => loop_info.vector_int_ops = event.value,
            EventType::RooflineVectorFloatOps => loop_info.vector_float_ops = event.value,
            EventType::RooflineVectorDoubleOps => loop_info.vector_double_ops = event.value,
            _ => {}
        }
        Ok(())
    }
}

/// Cumulative system-wide DRAM bytes recorded during the timed run, as a
/// step-free curve that can be integrated over an arbitrary time window.
struct BandwidthTimeline {
    samples: Vec<(u64, u64)>,
}

impl BandwidthTimeline {
    fn load(res_dir: &Path) -> Result<Option<Self>> {
        let samples =
            super::memory::parse_bandwidth_samples(&res_dir.join("memory-bandwidth.txt"))?
                .into_iter()
                .map(|sample| {
                    (
                        sample.timestamp,
                        sample.read_bytes.saturating_add(sample.write_bytes),
                    )
                })
                .collect::<Vec<_>>();
        Ok((samples.len() >= 2).then_some(Self { samples }))
    }

    fn cumulative_at(&self, timestamp: u64) -> f64 {
        match self
            .samples
            .binary_search_by_key(&timestamp, |(time, _)| *time)
        {
            Ok(index) => self.samples[index].1 as f64,
            Err(0) => self.samples[0].1 as f64,
            Err(index) if index == self.samples.len() => {
                self.samples[self.samples.len() - 1].1 as f64
            }
            Err(index) => {
                let (before_time, before_bytes) = self.samples[index - 1];
                let (after_time, after_bytes) = self.samples[index];
                let span = after_time.saturating_sub(before_time) as f64;
                let fraction = if span == 0.0 {
                    0.0
                } else {
                    (timestamp - before_time) as f64 / span
                };
                before_bytes as f64 + (after_bytes - before_bytes) as f64 * fraction
            }
        }
    }

    /// Bytes observed inside `intervals`, or `None` when nothing was observed
    /// and a measured denominator would be meaningless.
    fn bytes_in(&self, intervals: &[(u64, u64)]) -> Option<i64> {
        let bytes: f64 = intervals
            .iter()
            .map(|(start, end)| self.cumulative_at(*end) - self.cumulative_at(*start))
            .sum();
        (bytes >= 1.0).then_some(bytes as i64)
    }
}

/// Fold the recorded instrumentation events into `roofline_ops` and
/// `roofline_loop_runs`.
pub(crate) fn process_loops(
    tables: &Tables,
    record_info: &RecordInfo,
    res_dir: &Path,
) -> Result<()> {
    let ScenarioInfo::Roofline(info) = &record_info.scenario_info else {
        return Ok(());
    };
    let mut data = RooflineData {
        baseline_pid: info.perf_pid,
        instrumented_pid: info.inst_pid,
        loops: HashMap::new(),
        runs: Vec::new(),
        ops: Vec::new(),
    };

    if tables.has_table("events") {
        let names = utils::load_strings(tables.connection())?;
        let payloads = load_payloads(tables)?;
        let mut statement = tables.connection().prepare(
            "SELECT timestamp, event_id, instance, parent_id, flow_id, \"type\", pid, tid, value
             FROM events",
        )?;
        let events = statement
            .query_map([], |row| {
                Ok(TraceEvent {
                    timestamp: row.get(0)?,
                    event_id: row.get(1)?,
                    instance: row.get(2)?,
                    parent_id: row.get(3)?,
                    flow_id: row.get(4)?,
                    kind: row.get(5)?,
                    pid: row.get(6)?,
                    tid: row.get(7)?,
                    value: row.get(8)?,
                })
            })?
            .collect::<store::duckdb::Result<Vec<_>>>()?;
        for event in &events {
            data.consume(event, &payloads, &names)?;
        }
    }

    let mut runs = Columns::default();
    runs.u64(
        "unique_id",
        data.runs.iter().map(|(run, _)| run.id).collect(),
    );
    runs.i64(
        "process_id",
        data.runs.iter().map(|(run, _)| run.pid as i64).collect(),
    );
    runs.i64(
        "thread_id",
        data.runs.iter().map(|(run, _)| run.tid as i64).collect(),
    );
    runs.u64(
        "file_name",
        data.runs.iter().map(|(run, _)| run.file_name).collect(),
    );
    runs.u64(
        "function_name",
        data.runs.iter().map(|(run, _)| run.func_name).collect(),
    );
    runs.i64(
        "line",
        data.runs.iter().map(|(run, _)| run.line as i64).collect(),
    );
    runs.i64(
        "loop_start_ts",
        data.runs.iter().map(|(run, _)| run.start).collect(),
    );
    runs.i64(
        "loop_end_ts",
        data.runs.iter().map(|(_, end)| *end).collect(),
    );
    let bandwidth = BandwidthTimeline::load(res_dir)?;
    runs.i64_opt(
        "measured_dram_bytes",
        data.runs
            .iter()
            .map(|(run, end)| {
                bandwidth
                    .as_ref()
                    .and_then(|timeline| timeline.bytes_in(&[(run.start as u64, *end as u64)]))
            })
            .collect(),
    );
    tables.write("roofline_loop_runs", runs.finish()?)?;

    let mut ops = Columns::default();
    ops.u64("unique_id", data.ops.iter().map(|op| op.id).collect());
    ops.i64(
        "process_id",
        data.ops.iter().map(|op| op.pid as i64).collect(),
    );
    ops.i64(
        "thread_id",
        data.ops.iter().map(|op| op.tid as i64).collect(),
    );
    ops.u64(
        "file_name",
        data.ops.iter().map(|op| op.file_name).collect(),
    );
    ops.u64(
        "function_name",
        data.ops.iter().map(|op| op.func_name).collect(),
    );
    ops.i64("line", data.ops.iter().map(|op| op.line as i64).collect());
    ops.i64(
        "bytes_load",
        data.ops.iter().map(|op| op.bytes_load).collect(),
    );
    ops.i64(
        "bytes_store",
        data.ops.iter().map(|op| op.bytes_store).collect(),
    );
    ops.i64(
        "scalar_int_ops",
        data.ops.iter().map(|op| op.scalar_int_ops).collect(),
    );
    ops.i64(
        "scalar_float_ops",
        data.ops.iter().map(|op| op.scalar_float_ops).collect(),
    );
    ops.i64(
        "scalar_double_ops",
        data.ops.iter().map(|op| op.scalar_double_ops).collect(),
    );
    ops.i64(
        "vector_int_ops",
        data.ops.iter().map(|op| op.vector_int_ops).collect(),
    );
    ops.i64(
        "vector_float_ops",
        data.ops.iter().map(|op| op.vector_float_ops).collect(),
    );
    ops.i64(
        "vector_double_ops",
        data.ops.iter().map(|op| op.vector_double_ops).collect(),
    );
    tables.write("roofline_ops", ops.finish()?)?;

    Ok(())
}

fn load_payloads(tables: &Tables) -> Result<HashMap<u64, Payload>> {
    if !tables.has_table("payloads") {
        return Ok(HashMap::new());
    }
    let mut statement = tables
        .connection()
        .prepare("SELECT event_id, function_id, file_id, line FROM payloads")?;
    let payloads = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                Payload {
                    function_id: row.get(1)?,
                    file_id: row.get(2)?,
                    line: row.get(3)?,
                },
            ))
        })?
        .collect::<store::duckdb::Result<HashMap<_, _>>>()?;
    Ok(payloads)
}

#[derive(Deserialize)]
struct BinaryLoopFile {
    format_version: u32,
    executable: String,
    loops: Vec<BinaryLoop>,
}

#[derive(Deserialize)]
struct BinaryLoop {
    module_offset: String,
    function: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    trip_count: u64,
    block_ranges: Vec<BinaryBlockRange>,
    inclusive: BinaryLoopCounts,
}

#[derive(Deserialize)]
struct BinaryBlockRange {
    module_start: String,
    module_end: String,
}

#[derive(Default, Deserialize)]
struct BinaryLoopCounts {
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
    bytes_load: u64,
    bytes_store: u64,
    arch_bytes_load: u64,
    arch_bytes_store: u64,
    unclassified_instructions: u64,
}

struct ModuleMapping {
    runtime_start: u64,
    runtime_end: u64,
    svma_start: u64,
}

struct BinaryTimingSample {
    timestamp: i64,
    module_address: u64,
}

/// Attribute native sample time to the loops discovered by the binary backends.
/// The table is always written; it stays empty unless a binary backend both
/// discovered loops and measured them natively.
pub(crate) fn process_binary_loops(
    tables: &Tables,
    record_info: &RecordInfo,
    res_dir: &Path,
) -> Result<()> {
    let mut rows = BinaryLoopRows::default();
    if let Some(artifact) = binary_loop_artifact(record_info, res_dir)? {
        collect_binary_loops(tables, record_info, artifact, res_dir, &mut rows)?;
    }
    tables.write("roofline_binary_loops", rows.finish()?)
}

fn binary_loop_artifact(
    record_info: &RecordInfo,
    res_dir: &Path,
) -> Result<Option<BinaryLoopFile>> {
    let ScenarioInfo::Roofline(roofline_info) = &record_info.scenario_info else {
        return Ok(None);
    };
    let Some(method) = roofline_info.method.as_deref() else {
        return Ok(None);
    };
    if !matches!(method.accounting.as_str(), "qemu" | "dynamorio") || method.performance != "native"
    {
        return Ok(None);
    }
    let artifact_path = res_dir.join("qemu-roofline.loops.json");
    if !artifact_path.is_file() {
        anyhow::bail!(
            "native QEMU Roofline recording is missing '{}'",
            artifact_path.display()
        );
    }
    let artifact: BinaryLoopFile = serde_json::from_reader(
        std::fs::File::open(&artifact_path)
            .with_context(|| format!("open binary loop artifact '{}'", artifact_path.display()))?,
    )
    .with_context(|| format!("parse binary loop artifact '{}'", artifact_path.display()))?;
    if artifact.format_version != 3 {
        anyhow::bail!(
            "unsupported binary loop artifact version {}",
            artifact.format_version
        );
    }
    Ok(Some(artifact))
}

#[derive(Default)]
struct BinaryLoopRows {
    module_offset: Vec<String>,
    function_name: Vec<String>,
    file_name: Vec<String>,
    line: Vec<i64>,
    trip_count: Vec<i64>,
    duration: Vec<Option<i64>>,
    timing_samples: Vec<i64>,
    relative_errors: Vec<Option<f64>>,
    timing_qualities: Vec<String>,
    measured_dram_bytes: Vec<Option<i64>>,
    counters: [Vec<i64>; 10],
}

impl BinaryLoopRows {
    fn finish(self) -> Result<store::arrow::record_batch::RecordBatch> {
        let mut columns = Columns::default();
        columns.text("module_offset", self.module_offset);
        columns.text("function_name", self.function_name);
        columns.text("file_name", self.file_name);
        columns.i64("line", self.line);
        columns.i64("trip_count", self.trip_count);
        columns.i64_opt("duration_ns", self.duration);
        columns.i64("timing_samples", self.timing_samples);
        columns.f64_opt("timing_relative_error", self.relative_errors);
        columns.text("timing_quality", self.timing_qualities);
        columns.i64_opt("measured_dram_bytes", self.measured_dram_bytes);
        for (name, values) in [
            "bytes_load",
            "bytes_store",
            "arch_bytes_load",
            "arch_bytes_store",
            "scalar_int_ops",
            "scalar_float_ops",
            "scalar_double_ops",
            "vector_int_ops",
            "vector_float_ops",
            "vector_double_ops",
        ]
        .into_iter()
        .zip(self.counters)
        {
            columns.i64(name, values);
        }
        columns.finish()
    }
}

fn collect_binary_loops(
    tables: &Tables,
    record_info: &RecordInfo,
    artifact: BinaryLoopFile,
    res_dir: &Path,
    rows: &mut BinaryLoopRows,
) -> Result<()> {
    let ScenarioInfo::Roofline(roofline_info) = &record_info.scenario_info else {
        return Ok(());
    };
    let executable = Path::new(&artifact.executable);
    let object_data = std::fs::read(executable)
        .with_context(|| format!("read Roofline executable '{}'", executable.display()))?;
    let object = object::File::parse(object_data.as_slice())
        .with_context(|| format!("parse Roofline executable '{}'", executable.display()))?;
    let modules = utils::load_modules(tables.connection())?;
    let mappings =
        executable_mappings(&modules, roofline_info.perf_pid as u32, executable, &object);
    if mappings.is_empty() {
        anyhow::bail!(
            "native Roofline samples have no executable mapping for '{}'",
            executable.display()
        );
    }

    let mut samples = Vec::new();
    if tables
        .columns("pmu_counters")
        .iter()
        .any(|column| column == "os_cpu_clock")
    {
        let mut statement = tables.connection().prepare(
            "SELECT timestamp, ip FROM pmu_counters
             WHERE process_id = ? AND os_cpu_clock > 0 AND ip != 0
             ORDER BY timestamp",
        )?;
        let timings = statement
            .query_map([roofline_info.perf_pid as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, u64>(1)?))
            })?
            .collect::<store::duckdb::Result<Vec<_>>>()?;
        for (timestamp, ip) in timings {
            if let Some(module_address) = normalize_sample_ip(ip, &mappings) {
                samples.push(BinaryTimingSample {
                    timestamp,
                    module_address,
                });
            }
        }
    }

    let bandwidth = BandwidthTimeline::load(res_dir)?;
    let sample_period_ns = 1_000_000_000_u64
        .checked_div(record_info.sampling_frequency_hz.unwrap_or(1_000).max(1))
        .unwrap_or(1_000_000);

    for loop_info in artifact.loops {
        let ranges = loop_info
            .block_ranges
            .iter()
            .map(|range| {
                Ok((
                    parse_hex_address(&range.module_start)?,
                    parse_hex_address(&range.module_end)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let timestamps = samples
            .iter()
            .filter(|sample| {
                ranges.iter().any(|(start, end)| {
                    sample.module_address >= *start && sample.module_address < *end
                })
            })
            .map(|sample| sample.timestamp as u64)
            .collect::<Vec<_>>();
        let intervals = sampled_active_intervals(&timestamps, sample_period_ns);
        let sample_count = timestamps.len() as u64;
        // Treat samples as independent occupancy observations. The 95%
        // relative sampling error is approximately 1.96/sqrt(N). Only
        // rows at or below 10% are allowed to produce a throughput point.
        let relative_error = (sample_count > 0).then(|| 1.96 / (sample_count as f64).sqrt());
        let quality = match (
            loop_info.inclusive.unclassified_instructions == 0,
            relative_error,
        ) {
            (false, _) => "unclassified-instructions",
            (true, Some(error)) if error <= 0.10 => "high-confidence",
            (true, Some(error)) if error <= 0.20 => "low-confidence",
            _ => "insufficient-samples",
        };
        let counts = loop_info.inclusive;

        rows.module_offset.push(loop_info.module_offset);
        rows.function_name.push(
            loop_info
                .function
                .unwrap_or_else(|| "[unknown loop]".to_owned()),
        );
        rows.file_name.push(loop_info.file.unwrap_or_default());
        rows.line.push(loop_info.line.unwrap_or_default() as i64);
        rows.trip_count.push(loop_info.trip_count as i64);
        rows.duration
            .push((quality == "high-confidence").then(|| active_duration(&intervals) as i64));
        rows.timing_samples.push(sample_count as i64);
        rows.relative_errors.push(relative_error);
        rows.timing_qualities.push(quality.to_owned());
        rows.measured_dram_bytes.push(
            bandwidth
                .as_ref()
                .and_then(|timeline| timeline.bytes_in(&intervals)),
        );
        for (values, count) in rows.counters.iter_mut().zip([
            counts.bytes_load,
            counts.bytes_store,
            counts.arch_bytes_load,
            counts.arch_bytes_store,
            counts.scalar_int_ops,
            counts.scalar_float_ops,
            counts.scalar_double_ops,
            counts.vector_int_ops,
            counts.vector_float_ops,
            counts.vector_double_ops,
        ]) {
            values.push(count as i64);
        }
    }

    Ok(())
}

fn executable_mappings<'data>(
    modules: &[ProcMapEntry],
    pid: u32,
    executable: &Path,
    object: &object::File<'data>,
) -> Vec<ModuleMapping> {
    modules
        .iter()
        .filter(|mapping| mapping.pid == pid)
        .filter(|mapping| paths_refer_to_same_file(Path::new(&mapping.filename), executable))
        .filter_map(|mapping| {
            let mapping_offset = mapping.offset as u64;
            let segment = object
                .segments()
                .filter(|segment| {
                    let (file_offset, file_size) = segment.file_range();
                    mapping_offset >= (file_offset & !0xfff)
                        && mapping_offset < file_offset.saturating_add(file_size)
                })
                .min_by_key(|segment| segment.file_range().0.abs_diff(mapping_offset))?;
            let (file_offset, _) = segment.file_range();
            Some(ModuleMapping {
                runtime_start: mapping.address as u64,
                runtime_end: mapping.address.saturating_add(mapping.size) as u64,
                svma_start: segment
                    .address()
                    .saturating_add(mapping_offset)
                    .saturating_sub(file_offset),
            })
        })
        .collect()
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    let left_text = left.to_string_lossy();
    let left = Path::new(
        left_text
            .strip_suffix(" (deleted)")
            .unwrap_or(left_text.as_ref()),
    );
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

fn normalize_sample_ip(ip: u64, mappings: &[ModuleMapping]) -> Option<u64> {
    mappings
        .iter()
        .find(|mapping| ip >= mapping.runtime_start && ip < mapping.runtime_end)
        .map(|mapping| {
            mapping
                .svma_start
                .saturating_add(ip.saturating_sub(mapping.runtime_start))
        })
}

/// The disjoint time windows a loop occupied, each sample standing for the
/// sampling period that ended at it.
fn sampled_active_intervals(timestamps: &[u64], period_ns: u64) -> Vec<(u64, u64)> {
    let mut observations = timestamps
        .iter()
        .map(|timestamp| (timestamp.saturating_sub(period_ns), *timestamp))
        .collect::<Vec<_>>();
    observations.sort_unstable();
    let mut intervals: Vec<(u64, u64)> = Vec::new();
    for (start, end) in observations {
        match intervals.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => intervals.push((start, end)),
        }
    }
    intervals
}

fn active_duration(intervals: &[(u64, u64)]) -> u64 {
    intervals.iter().fold(0_u64, |total, (start, end)| {
        total.saturating_add(end.saturating_sub(*start))
    })
}

fn parse_hex_address(value: &str) -> Result<u64> {
    let value = value
        .strip_prefix("0x")
        .with_context(|| format!("binary loop address is not hexadecimal: '{value}'"))?;
    u64::from_str_radix(value, 16)
        .with_context(|| format!("invalid binary loop address '0x{value}'"))
}

/// The Roofline chart table: instrumented loop runs, or the binary loops when
/// a binary backend measured them.
pub(crate) fn write_chart(tables: &Tables) -> Result<()> {
    tables.write_query(
        "roofline",
        "WITH
         ops AS (
           SELECT process_id, file_name, function_name, line,
             SUM(bytes_load) AS bytes_load, SUM(bytes_store) AS bytes_store,
             SUM(scalar_int_ops) AS scalar_int_ops,
             SUM(scalar_float_ops) AS scalar_float_ops,
             SUM(scalar_double_ops) AS scalar_double_ops,
             SUM(vector_int_ops) AS vector_int_ops,
             SUM(vector_float_ops) AS vector_float_ops,
             SUM(vector_double_ops) AS vector_double_ops
           FROM roofline_ops
           GROUP BY process_id, file_name, function_name, line
         ),
         runs AS (
           SELECT process_id, file_name, function_name, line,
             SUM(loop_end_ts - loop_start_ts) AS total_duration,
             SUM(measured_dram_bytes) AS measured_dram_bytes
           FROM roofline_loop_runs
           GROUP BY process_id, file_name, function_name, line
         )
         SELECT
           s_file.string AS file_name,
           s_func.string AS function_name,
           runs.line AS line,
           CAST(ops.scalar_int_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_int_ops,
           CAST(ops.scalar_int_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS scalar_int_ai,
           CAST(ops.scalar_float_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_float_ops,
           CAST(ops.scalar_float_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS scalar_float_ai,
           CAST(ops.scalar_double_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_double_ops,
           CAST(ops.scalar_double_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS scalar_double_ai,
           CAST(ops.vector_int_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_int_ops,
           CAST(ops.vector_int_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS vector_int_ai,
           CAST(ops.vector_float_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_float_ops,
           CAST(ops.vector_float_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS vector_float_ai,
           CAST(ops.vector_double_ops AS DOUBLE) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_double_ops,
           CAST(ops.vector_double_ops AS DOUBLE) / NULLIF(COALESCE(runs.measured_dram_bytes, ops.bytes_load + ops.bytes_store), 0) AS vector_double_ai,
           CAST(NULL AS BIGINT) AS timing_samples,
           CAST(NULL AS DOUBLE) AS timing_relative_error,
           'legacy' AS timing_quality,
           CAST(NULL AS VARCHAR) AS module_offset,
           CAST(NULL AS BIGINT) AS trip_count,
           CAST(ops.bytes_load + ops.bytes_store AS BIGINT) AS arch_bytes,
           CAST(NULL AS BIGINT) AS dram_bytes,
           CAST(runs.measured_dram_bytes AS BIGINT) AS measured_dram_bytes,
           CASE WHEN runs.measured_dram_bytes IS NULL THEN 'architectural'
                ELSE 'uncore_measured' END AS traffic_source
         FROM runs
         LEFT JOIN ops
           ON runs.file_name = ops.file_name
           AND runs.function_name = ops.function_name
           AND runs.line = ops.line
         LEFT JOIN strings s_file ON runs.file_name = s_file.id
         LEFT JOIN strings s_func ON runs.function_name = s_func.id
         WHERE NOT EXISTS (SELECT 1 FROM roofline_binary_loops)

         UNION ALL

         SELECT
           file_name,
           function_name,
           line,
           CAST(scalar_int_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(scalar_int_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           CAST(scalar_float_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(scalar_float_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           CAST(scalar_double_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(scalar_double_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           CAST(vector_int_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(vector_int_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           CAST(vector_float_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(vector_float_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           CAST(vector_double_ops AS DOUBLE) * 1000000000.0 / NULLIF(duration_ns, 0),
           CAST(vector_double_ops AS DOUBLE) / NULLIF(COALESCE(measured_dram_bytes, arch_bytes_load + arch_bytes_store), 0),
           timing_samples,
           timing_relative_error,
           timing_quality,
           module_offset,
           trip_count,
           arch_bytes_load + arch_bytes_store,
           bytes_load + bytes_store,
           measured_dram_bytes,
           CASE WHEN measured_dram_bytes IS NULL THEN 'architectural'
                ELSE 'uncore_measured' END
         FROM roofline_binary_loops",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postprocess::tables::Columns;

    #[test]
    fn bandwidth_timeline_integrates_over_kernel_windows() {
        let timeline = BandwidthTimeline {
            samples: vec![(0, 0), (1_000, 1_000), (2_000, 5_000)],
        };
        assert_eq!(timeline.bytes_in(&[(0, 2_000)]), Some(5_000));
        assert_eq!(timeline.bytes_in(&[(1_000, 1_500)]), Some(2_000));
        // Windows outside the recorded curve contribute nothing.
        assert_eq!(timeline.bytes_in(&[(3_000, 4_000)]), None);
    }

    /// The single chart row produced for one binary loop with 5000
    /// architectural bytes, 1000 modeled DRAM bytes, and `measured` bytes
    /// counted on the memory controller.
    fn chart_row(measured: Option<i64>) -> (String, f64, f64, String, Option<i64>, String) {
        let dir = tempfile::tempdir().unwrap();
        let tables = Tables::open(dir.path()).unwrap();
        for table in ["roofline_ops", "roofline_loop_runs"] {
            let mut columns = Columns::default();
            columns.u64("file_name", vec![]);
            columns.u64("function_name", vec![]);
            columns.i64("process_id", vec![]);
            columns.i64("line", vec![]);
            columns.i64("bytes_load", vec![]);
            columns.i64("bytes_store", vec![]);
            columns.i64("scalar_int_ops", vec![]);
            columns.i64("scalar_float_ops", vec![]);
            columns.i64("scalar_double_ops", vec![]);
            columns.i64("vector_int_ops", vec![]);
            columns.i64("vector_float_ops", vec![]);
            columns.i64("vector_double_ops", vec![]);
            columns.i64("loop_start_ts", vec![]);
            columns.i64("loop_end_ts", vec![]);
            columns.i64_opt("measured_dram_bytes", vec![]);
            tables.write(table, columns.finish().unwrap()).unwrap();
        }
        let mut strings = Columns::default();
        strings.u64("id", vec![]);
        strings.text("string", vec![]);
        tables.write("strings", strings.finish().unwrap()).unwrap();

        let mut rows = BinaryLoopRows::default();
        rows.module_offset.push("0x100".to_owned());
        rows.function_name.push("kernel".to_owned());
        rows.file_name.push("kernel.c".to_owned());
        rows.line.push(7);
        rows.trip_count.push(100);
        rows.duration.push(Some(1_000_000_000));
        rows.timing_samples.push(400);
        rows.relative_errors.push(Some(0.098));
        rows.timing_qualities.push("high-confidence".to_owned());
        rows.measured_dram_bytes.push(measured);
        // bytes_load, bytes_store, arch_bytes_load, arch_bytes_store, then ops.
        for (index, value) in [
            800,
            200,
            4000,
            1000,
            0,
            0,
            2_000_000_000,
            0,
            0,
            6_000_000_000,
        ]
        .into_iter()
        .enumerate()
        {
            rows.counters[index].push(value);
        }
        tables
            .write("roofline_binary_loops", rows.finish().unwrap())
            .unwrap();

        write_chart(&tables).unwrap();
        tables
            .connection()
            .query_row(
                "SELECT function_name, scalar_double_ai, vector_double_ai, timing_quality,
                        measured_dram_bytes, traffic_source
                 FROM roofline",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap()
    }

    #[test]
    fn binary_loops_replace_the_instrumented_rows() {
        let (function, scalar_ai, vector_ai, quality, measured, source) = chart_row(None);
        assert_eq!(function, "kernel");
        // Without measured traffic the denominator is the 5000 architectural
        // bytes, not the 1000 bytes the model says reached DRAM.
        assert_eq!(scalar_ai, 400_000.0);
        assert_eq!(vector_ai, 1_200_000.0);
        assert_eq!(quality, "high-confidence");
        assert_eq!(measured, None);
        assert_eq!(source, "architectural");
    }

    #[test]
    fn measured_dram_bytes_become_the_intensity_denominator() {
        let (_, scalar_ai, vector_ai, _, measured, source) = chart_row(Some(2_000));
        assert_eq!(scalar_ai, 1_000_000.0);
        assert_eq!(vector_ai, 3_000_000.0);
        assert_eq!(measured, Some(2_000));
        assert_eq!(source, "uncore_measured");
    }

    #[test]
    fn sampled_duration_unions_parallel_and_separated_observations() {
        let duration =
            |timestamps: &[u64]| active_duration(&sampled_active_intervals(timestamps, 1_000));
        assert_eq!(duration(&[1_000, 1_500]), 1_500);
        assert_eq!(duration(&[1_000, 3_000]), 2_000);
        assert_eq!(duration(&[]), 0);
    }
}
