use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use anyhow::{Context, Result};
use kdam::BarExt;
use memmap2::{Advice, Mmap};
use mperf_data::{
    CallFrame, CpuClockSource, Event, EventType, IString, ProcMapEntry, RecordInfo, Scenario,
    ScenarioInfo,
};
use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use serde::Deserialize;
use smallvec::SmallVec;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

use crate::disassembly::{DisassembleRequest, DisassembleTarget, default_disassembler};
use crate::utils;

/// A core cluster resolved for post-processing: `(family_id, display name,
/// inclusive CPU ranges)`.
type ClusterRanges = (String, String, Vec<(u32, u32)>);

pub async fn perform_postprocessing(res_dir: &Path, pb: kdam::Bar) -> Result<()> {
    let mut pb = pb;

    let data = fs::read_to_string(res_dir.join("info.json"))
        .await
        .expect("failed to read info.json");
    let info: RecordInfo = serde_json::from_str(&data).expect("failed to parse info.json");

    let connection = sqlite::open(res_dir.join("perf.db"))?;
    connection.execute(
        "
            CREATE TABLE proc_map (
                ip INTEGER,
                func_name TEXT,
                file_name TEXT,
                line INTEGER,
                module_path TEXT
            );
            CREATE TABLE strings (id BINARY(128) NOT NULL, string TEXT NOT NULL);
        ",
    )?;

    process_strings(&connection, res_dir).await?;

    match info.scenario {
        Scenario::Snapshot => {
            process_pmu_counters(&connection, &info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
        }
        Scenario::Mem => {
            process_pmu_counters(&connection, &info, res_dir, &mut pb).await?;
            process_memory_artifact(&connection, &info, res_dir)?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
        }
        Scenario::Roofline => {
            process_pmu_counters(&connection, &info, res_dir, &mut pb).await?;
            if res_dir.join("qemu-roofline.memory.json").exists() {
                process_memory_artifact(&connection, &info, res_dir)?;
            }
            process_binary_roofline_loops(&connection, &info, res_dir)?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
            create_roofline_view(&connection).await?;
        }
        Scenario::TMA => {
            process_pmu_counters(&connection, &info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_tma_view(&connection, &info.scenario_info).await?;
        }
    }

    persist_derived_metrics(&connection)?;

    Ok(())
}

#[derive(Deserialize)]
struct MemoryArtifact {
    format_version: u32,
    line_size: u64,
    references: u64,
    architectural_load_bytes: u64,
    architectural_store_bytes: u64,
    unique_lines: u64,
    distinct_bytes: u64,
    cold_references: u64,
    reuse_distance_log2: BTreeMap<u32, u64>,
    spatial_utilization_percent: BTreeMap<u32, u64>,
    stride_lines_log2: BTreeMap<i32, u64>,
    working_set: Vec<MemoryWorkingSetArtifact>,
}

#[derive(Deserialize)]
struct MemoryWorkingSetArtifact {
    window_references: u64,
    mean_lines: f64,
    p95_lines: u64,
    max_lines: u64,
}

#[derive(Deserialize)]
struct NativeTimingArtifact {
    pid: i32,
    start_ns: u64,
    end_ns: u64,
}

fn process_memory_artifact(
    connection: &sqlite::Connection,
    record_info: &RecordInfo,
    res_dir: &Path,
) -> Result<()> {
    let artifact_path = res_dir.join("qemu-roofline.memory.json");
    let artifact: MemoryArtifact = serde_json::from_reader(
        std::fs::File::open(&artifact_path)
            .with_context(|| format!("open memory artifact '{}'", artifact_path.display()))?,
    )
    .with_context(|| format!("parse memory artifact '{}'", artifact_path.display()))?;
    if artifact.format_version != 1 {
        anyhow::bail!(
            "unsupported memory artifact version {}",
            artifact.format_version
        );
    }
    let timing: NativeTimingArtifact = serde_json::from_reader(
        std::fs::File::open(res_dir.join("memory-native.json"))
            .context("open native memory timing artifact")?,
    )
    .context("parse native memory timing artifact")?;
    let duration_ns = timing.end_ns.saturating_sub(timing.start_ns);
    let counts = parse_counter_file(&res_dir.join("qemu-roofline.counts"))?;
    let modeled_load = counts.get("dram_bytes_load").copied().unwrap_or(0);
    let modeled_store = counts.get("dram_bytes_store").copied().unwrap_or(0);
    let modeled_total = modeled_load.saturating_add(modeled_store);
    let bandwidth_samples = parse_bandwidth_samples(&res_dir.join("memory-bandwidth.txt"))?;
    let hardware_total = bandwidth_samples
        .last()
        .map(|sample| sample.read_bytes.saturating_add(sample.write_bytes));
    let primary_total = hardware_total.unwrap_or(modeled_total);
    let (bandwidth_source, bandwidth_scope) = if hardware_total.is_some() {
        ("hardware_memory_controller", "system_during_target")
    } else {
        ("process_modeled", "process")
    };
    let achieved_gbytes_per_second =
        (duration_ns != 0).then(|| primary_total as f64 / duration_ns as f64);
    let calibration = record_info.cpu_info.memory_calibration.as_deref();
    let peak = calibration.map(|value| value.gbytes_per_second);
    let utilization = achieved_gbytes_per_second
        .zip(peak)
        .filter(|(_, peak)| *peak > 0.0)
        .map(|(achieved, peak)| achieved / peak);
    let cold_fraction = (artifact.references != 0)
        .then(|| artifact.cold_references as f64 / artifact.references as f64);

    connection.execute(
        "CREATE TABLE memory_summary (
            format_version INTEGER NOT NULL,
            process_id INTEGER NOT NULL,
            line_size INTEGER NOT NULL,
            reference_count INTEGER NOT NULL,
            architectural_load_bytes INTEGER NOT NULL,
            architectural_store_bytes INTEGER NOT NULL,
            unique_lines INTEGER NOT NULL,
            accessed_footprint_bytes INTEGER NOT NULL,
            cold_references INTEGER NOT NULL,
            cold_fraction REAL,
            modeled_dram_read_bytes INTEGER NOT NULL,
            modeled_dram_write_bytes INTEGER NOT NULL,
            native_duration_ns INTEGER NOT NULL,
            achieved_gbytes_per_second REAL,
            peak_gbytes_per_second REAL,
            bandwidth_utilization REAL,
            bandwidth_source TEXT NOT NULL,
            bandwidth_scope TEXT NOT NULL,
            live_allocated_bytes INTEGER,
            peak_allocated_bytes INTEGER,
            live_mapped_bytes INTEGER,
            peak_rss_bytes INTEGER,
            quality TEXT NOT NULL
        );
        CREATE TABLE memory_timeline (
            timestamp_ns INTEGER NOT NULL,
            live_allocated_bytes INTEGER,
            live_mapped_bytes INTEGER,
            rss_bytes INTEGER,
            dram_read_gbytes_per_second REAL,
            dram_write_gbytes_per_second REAL,
            bandwidth_source TEXT
        );
        CREATE TABLE memory_working_set (
            window_references INTEGER NOT NULL,
            mean_bytes REAL NOT NULL,
            p95_bytes INTEGER NOT NULL,
            max_bytes INTEGER NOT NULL
        );
        CREATE TABLE memory_spatial_utilization (
            utilization_percent INTEGER NOT NULL,
            lines INTEGER NOT NULL
        );
        CREATE TABLE memory_strides (
            stride_log2_lines INTEGER NOT NULL,
            reference_count INTEGER NOT NULL
        );
        CREATE TABLE memory_reuse_distance (
            distance_log2_lines INTEGER NOT NULL,
            reference_count INTEGER NOT NULL
        );
        CREATE TABLE memory_miss_ratio (
            cache_lines INTEGER NOT NULL,
            cache_bytes INTEGER NOT NULL,
            miss_ratio REAL NOT NULL
        );",
    )?;

    let quality = match &record_info.scenario_info {
        ScenarioInfo::Mem(info) => info.method.quality.as_str(),
        ScenarioInfo::Roofline(info) => info
            .method
            .as_deref()
            .map_or("legacy", |method| method.quality.as_str()),
        _ => "unknown",
    };
    let mut insert = connection.prepare(
        "INSERT INTO memory_summary (
            format_version, process_id, line_size, reference_count,
            architectural_load_bytes, architectural_store_bytes, unique_lines,
            accessed_footprint_bytes, cold_references, cold_fraction,
            modeled_dram_read_bytes, modeled_dram_write_bytes, native_duration_ns,
            achieved_gbytes_per_second, peak_gbytes_per_second,
            bandwidth_utilization, bandwidth_source, bandwidth_scope, quality
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
    )?;
    insert.bind((1, artifact.format_version as i64))?;
    insert.bind((2, timing.pid as i64))?;
    insert.bind((3, sqlite_u64(artifact.line_size)))?;
    insert.bind((4, sqlite_u64(artifact.references)))?;
    insert.bind((5, sqlite_u64(artifact.architectural_load_bytes)))?;
    insert.bind((6, sqlite_u64(artifact.architectural_store_bytes)))?;
    insert.bind((7, sqlite_u64(artifact.unique_lines)))?;
    insert.bind((8, sqlite_u64(artifact.distinct_bytes)))?;
    insert.bind((9, sqlite_u64(artifact.cold_references)))?;
    insert.bind((10, cold_fraction))?;
    insert.bind((11, sqlite_u64(modeled_load)))?;
    insert.bind((12, sqlite_u64(modeled_store)))?;
    insert.bind((13, sqlite_u64(duration_ns)))?;
    insert.bind((14, achieved_gbytes_per_second))?;
    insert.bind((15, peak))?;
    insert.bind((16, utilization))?;
    insert.bind((17, bandwidth_source))?;
    insert.bind((18, bandwidth_scope))?;
    insert.bind((19, quality))?;
    insert.next()?;

    let rss_samples = parse_rss_samples(&res_dir.join("memory-rss.txt"))?;
    let peak_rss = rss_samples.iter().map(|(_, rss)| *rss).max();
    if let Some(peak_rss) = peak_rss {
        let mut update = connection.prepare("UPDATE memory_summary SET peak_rss_bytes = ?;")?;
        update.bind((1, sqlite_u64(peak_rss)))?;
        update.next()?;
    }
    let mut timeline = connection.prepare(
        "INSERT INTO memory_timeline (
            timestamp_ns, live_allocated_bytes, live_mapped_bytes, rss_bytes,
            dram_read_gbytes_per_second, dram_write_gbytes_per_second, bandwidth_source
         ) VALUES (?, NULL, NULL, ?, NULL, NULL, NULL);",
    )?;
    for (timestamp, rss) in rss_samples {
        timeline.reset()?;
        timeline.bind((1, sqlite_u64(timestamp)))?;
        timeline.bind((2, sqlite_u64(rss)))?;
        timeline.next()?;
    }
    let mut bandwidth_timeline = connection.prepare(
        "INSERT INTO memory_timeline (
            timestamp_ns, live_allocated_bytes, live_mapped_bytes, rss_bytes,
            dram_read_gbytes_per_second, dram_write_gbytes_per_second, bandwidth_source
         ) VALUES (?, NULL, NULL, NULL, ?, ?, 'hardware_memory_controller');",
    )?;
    for pair in bandwidth_samples.windows(2) {
        let elapsed = pair[1].timestamp.saturating_sub(pair[0].timestamp);
        if elapsed == 0 {
            continue;
        }
        let read = pair[1].read_bytes.saturating_sub(pair[0].read_bytes) as f64 / elapsed as f64;
        let write = pair[1].write_bytes.saturating_sub(pair[0].write_bytes) as f64 / elapsed as f64;
        bandwidth_timeline.reset()?;
        bandwidth_timeline.bind((1, sqlite_u64(pair[1].timestamp)))?;
        bandwidth_timeline.bind((2, read))?;
        bandwidth_timeline.bind((3, write))?;
        bandwidth_timeline.next()?;
    }
    if let Some(allocations) = parse_allocation_samples(&res_dir.join("memory-allocations.txt"))? {
        let mut update = connection.prepare(
            "UPDATE memory_summary SET
                live_allocated_bytes = ?, peak_allocated_bytes = ?, live_mapped_bytes = ?;",
        )?;
        update.bind((1, sqlite_u64(allocations.live_allocated)))?;
        update.bind((2, sqlite_u64(allocations.peak_allocated)))?;
        update.bind((3, sqlite_u64(allocations.live_mapped)))?;
        update.next()?;
        if allocations.child_seen {
            connection
                .execute("UPDATE memory_summary SET quality = quality || '+children-excluded';")?;
        }
        let mut allocation_timeline = connection.prepare(
            "INSERT INTO memory_timeline (
                timestamp_ns, live_allocated_bytes, live_mapped_bytes, rss_bytes,
                dram_read_gbytes_per_second, dram_write_gbytes_per_second, bandwidth_source
             ) VALUES (?, ?, ?, NULL, NULL, NULL, NULL);",
        )?;
        for point in allocations.points {
            allocation_timeline.reset()?;
            allocation_timeline.bind((1, sqlite_u64(point.timestamp)))?;
            allocation_timeline.bind((2, sqlite_u64(point.live_allocated)))?;
            allocation_timeline.bind((3, sqlite_u64(point.live_mapped)))?;
            allocation_timeline.next()?;
        }
    }

    let mut statement =
        connection.prepare("INSERT INTO memory_working_set VALUES (?, ?, ?, ?);")?;
    for window in artifact.working_set {
        statement.reset()?;
        statement.bind((1, sqlite_u64(window.window_references)))?;
        statement.bind((2, window.mean_lines * artifact.line_size as f64))?;
        statement.bind((
            3,
            sqlite_u64(window.p95_lines.saturating_mul(artifact.line_size)),
        ))?;
        statement.bind((
            4,
            sqlite_u64(window.max_lines.saturating_mul(artifact.line_size)),
        ))?;
        statement.next()?;
    }
    insert_histogram(
        connection,
        "memory_spatial_utilization",
        artifact.spatial_utilization_percent,
    )?;
    insert_histogram(connection, "memory_strides", artifact.stride_lines_log2)?;
    insert_histogram(
        connection,
        "memory_reuse_distance",
        artifact.reuse_distance_log2.clone(),
    )?;

    let mut miss = connection.prepare("INSERT INTO memory_miss_ratio VALUES (?, ?, ?);")?;
    for power in 0..=30_u32 {
        let lines = 1_u64 << power;
        let hits = artifact
            .reuse_distance_log2
            .iter()
            .filter(|(bucket, _)| **bucket <= power)
            .map(|(_, count)| *count)
            .sum::<u64>();
        let ratio = if artifact.references == 0 {
            0.0
        } else {
            1.0 - hits as f64 / artifact.references as f64
        };
        miss.reset()?;
        miss.bind((1, sqlite_u64(lines)))?;
        miss.bind((2, sqlite_u64(lines.saturating_mul(artifact.line_size))))?;
        miss.bind((3, ratio))?;
        miss.next()?;
    }
    Ok(())
}

fn parse_rss_samples(path: &Path) -> Result<Vec<(u64, u64)>> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    contents
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let timestamp = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("RSS sample has no timestamp"))?
                .parse()?;
            let rss = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("RSS sample has no value"))?
                .parse()?;
            Ok((timestamp, rss))
        })
        .collect()
}

struct BandwidthSample {
    timestamp: u64,
    read_bytes: u64,
    write_bytes: u64,
}

fn parse_bandwidth_samples(path: &Path) -> Result<Vec<BandwidthSample>> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    contents
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            Ok(BandwidthSample {
                timestamp: fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("bandwidth sample has no timestamp"))?
                    .parse()?,
                read_bytes: fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("bandwidth sample has no read count"))?
                    .parse()?,
                write_bytes: fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("bandwidth sample has no write count"))?
                    .parse()?,
            })
        })
        .collect()
}

struct AllocationPoint {
    timestamp: u64,
    live_allocated: u64,
    live_mapped: u64,
}

struct AllocationSummary {
    live_allocated: u64,
    peak_allocated: u64,
    live_mapped: u64,
    points: Vec<AllocationPoint>,
    child_seen: bool,
}

fn parse_allocation_samples(path: &Path) -> Result<Option<AllocationSummary>> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let mut heap = HashMap::<u64, u64>::new();
    let mut mappings = BTreeMap::<u64, u64>::new();
    let mut live_allocated = 0_u64;
    let mut peak_allocated = 0_u64;
    let mut live_mapped = 0_u64;
    let mut points = Vec::new();
    let mut child_seen = false;
    for line in contents.lines() {
        let mut fields = line.split_whitespace();
        let operation = fields
            .next()
            .and_then(|value| value.as_bytes().first())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("invalid allocation event '{line}'"))?;
        let timestamp = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("allocation event has no timestamp"))?
            .parse::<u64>()?;
        let first = u64::from_str_radix(
            fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("allocation event has no address"))?,
            16,
        )?;
        let second = u64::from_str_radix(
            fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("allocation event has no second address"))?,
            16,
        )?;
        let size = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("allocation event has no size"))?
            .parse::<u64>()?;
        match operation {
            b'A' if first != 0 => {
                if let Some(old) = heap.insert(first, size) {
                    live_allocated = live_allocated.saturating_sub(old);
                }
                live_allocated = live_allocated.saturating_add(size);
            }
            b'F' => {
                if let Some(old) = heap.remove(&first) {
                    live_allocated = live_allocated.saturating_sub(old);
                }
            }
            b'R' if second != 0 => {
                if let Some(old) = heap.remove(&first) {
                    live_allocated = live_allocated.saturating_sub(old);
                }
                if let Some(old) = heap.insert(second, size) {
                    live_allocated = live_allocated.saturating_sub(old);
                }
                live_allocated = live_allocated.saturating_add(size);
            }
            b'M' if first != 0 => {
                if let Some(old) = mappings.insert(first, size) {
                    live_mapped = live_mapped.saturating_sub(old);
                }
                live_mapped = live_mapped.saturating_add(size);
            }
            b'U' => unmap_range(&mut mappings, first, size, &mut live_mapped),
            b'C' => child_seen = true,
            _ => {}
        }
        peak_allocated = peak_allocated.max(live_allocated);
        points.push(AllocationPoint {
            timestamp,
            live_allocated,
            live_mapped,
        });
    }
    Ok(Some(AllocationSummary {
        live_allocated,
        peak_allocated,
        live_mapped,
        points,
        child_seen,
    }))
}

fn unmap_range(mappings: &mut BTreeMap<u64, u64>, start: u64, size: u64, live: &mut u64) {
    let end = start.saturating_add(size);
    let overlaps = mappings
        .range(..end)
        .filter_map(|(&base, &length)| {
            (base.saturating_add(length) > start).then_some((base, length))
        })
        .collect::<Vec<_>>();
    for (base, length) in overlaps {
        mappings.remove(&base);
        let mapping_end = base.saturating_add(length);
        let removed_start = base.max(start);
        let removed_end = mapping_end.min(end);
        *live = live.saturating_sub(removed_end.saturating_sub(removed_start));
        if base < start {
            mappings.insert(base, start - base);
        }
        if mapping_end > end {
            mappings.insert(end, mapping_end - end);
        }
    }
}

#[cfg(test)]
mod memory_profile_tests {
    use super::*;

    #[test]
    fn allocation_replay_tracks_realloc_free_and_partial_unmap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("allocations.txt");
        std::fs::write(
            &path,
            "A 1 1000 0 64\nA 2 2000 0 32\nR 3 1000 3000 128\nF 4 2000 0 0\nM 5 4000 0 4096\nU 6 4400 0 1024\n",
        )
        .unwrap();
        let summary = parse_allocation_samples(&path).unwrap().unwrap();
        assert_eq!(summary.live_allocated, 128);
        assert_eq!(summary.peak_allocated, 160);
        assert_eq!(summary.live_mapped, 3072);
        assert_eq!(summary.points.len(), 6);
    }
}

fn parse_counter_file(path: &Path) -> Result<HashMap<String, u64>> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read counter artifact '{}'", path.display()))?;
    contents
        .lines()
        .map(|line| {
            let (name, value) = line
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("invalid counter line '{line}'"))?;
            Ok((name.to_string(), value.parse::<u64>()?))
        })
        .collect()
}

fn insert_histogram<K>(
    connection: &sqlite::Connection,
    table: &str,
    values: BTreeMap<K, u64>,
) -> Result<()>
where
    K: Into<i64> + Copy,
{
    let mut statement = connection.prepare(format!("INSERT INTO {table} VALUES (?, ?);"))?;
    for (bucket, count) in values {
        statement.reset()?;
        statement.bind((1, bucket.into()))?;
        statement.bind((2, sqlite_u64(count)))?;
        statement.next()?;
    }
    Ok(())
}

fn persist_derived_metrics(connection: &sqlite::Connection) -> Result<()> {
    persist_metric_definitions(connection, &pmu::host_metrics())
}

fn persist_metric_definitions(
    connection: &sqlite::Connection,
    metrics: &[pmu::Metric],
) -> Result<()> {
    use sqlite::State;

    connection.execute(
        "CREATE TABLE IF NOT EXISTS derived_metrics (
            name TEXT PRIMARY KEY,
            value REAL NOT NULL,
            unit TEXT,
            expression TEXT NOT NULL
        );",
    )?;

    for metric in metrics {
        let Ok(event_names) = metric.expression.event_names() else {
            continue;
        };
        let mut values = HashMap::new();
        let mut applicable = true;
        for event_name in event_names {
            let Some(column) = metric_event_column(&event_name) else {
                applicable = false;
                break;
            };
            let mut statement = match connection.prepare(format!(
                "SELECT SUM({column}) AS metric_value FROM pmu_counters"
            )) {
                Ok(statement) => statement,
                Err(_) => {
                    applicable = false;
                    break;
                }
            };
            if statement.next()? != State::Row {
                applicable = false;
                break;
            }
            let value = statement.read::<Option<i64>, _>("metric_value")?;
            values.insert(event_name, value.unwrap_or(0) as f64);
        }
        if !applicable {
            continue;
        }
        let Ok(value) = metric.expression.evaluate(&values) else {
            continue;
        };
        let mut insert = connection.prepare(
            "INSERT OR REPLACE INTO derived_metrics (name, value, unit, expression)
             VALUES (?, ?, ?, ?)",
        )?;
        insert.bind((1, metric.name.as_str()))?;
        insert.bind((2, value))?;
        insert.bind((3, metric.unit.as_deref()))?;
        insert.bind((4, metric.expression.0.as_str()))?;
        insert.next()?;
    }
    Ok(())
}

fn metric_event_column(name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("cycles") {
        Some("pmu_cycles")
    } else if name.eq_ignore_ascii_case("instructions") {
        Some("pmu_instructions")
    } else if name.eq_ignore_ascii_case("branches") {
        Some("pmu_branch_instructions")
    } else if name.eq_ignore_ascii_case("branch_misses") {
        Some("pmu_branch_misses")
    } else if name.eq_ignore_ascii_case("llc_references")
        || name.eq_ignore_ascii_case("cache_references")
    {
        Some("pmu_llc_references")
    } else if name.eq_ignore_ascii_case("llc_misses") || name.eq_ignore_ascii_case("cache_misses") {
        Some("pmu_llc_misses")
    } else if name.eq_ignore_ascii_case("stalled_cycles_frontend") {
        Some("pmu_stalled_cycles_frontend")
    } else if name.eq_ignore_ascii_case("stalled_cycles_backend") {
        Some("pmu_stalled_cycles_backend")
    } else {
        None
    }
}

async fn process_strings(connection: &sqlite::Connection, res_dir: &Path) -> Result<()> {
    let strings_file =
        std::fs::File::open(res_dir.join("strings.json")).expect("failed to open strings.json");
    let strings: Vec<IString> =
        serde_json::from_reader(strings_file).expect("failed to parse strings.json");

    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;
    let result = (|| -> Result<()> {
        let mut statement =
            connection.prepare("INSERT INTO strings (id, string) VALUES (?, ?);")?;
        for s in strings {
            statement.reset()?;
            bind_id(&mut statement, 1, s.id)?;
            statement.bind((2, s.value.as_str()))?;
            statement.next()?;
        }
        Ok(())
    })();
    finish_transaction(connection, result)?;

    Ok(())
}

fn finish_transaction(connection: &sqlite::Connection, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            connection.execute("COMMIT;")?;
            Ok(())
        }
        Err(error) => {
            let _ = connection.execute("ROLLBACK;");
            Err(error)
        }
    }
}

fn get_event_column_name(event: &(EventType, String)) -> String {
    match event.0 {
        EventType::PmuCustom => format!("pmu_{}", event.1.replace('.', "_")),
        _ => event.0.to_string(),
    }
}

#[derive(Clone)]
struct CounterLead {
    unique_id: u128,
    correlation_id: u128,
    process_id: u32,
    thread_id: u32,
    cpu: u32,
    time_enabled: u64,
    time_running: u64,
    timestamp: u64,
    callstack: SmallVec<[CallFrame; 32]>,
}

#[derive(Clone)]
struct ResolvedIp {
    functions: Vec<String>,
    function: String,
    file: String,
    line: u32,
    module_path: Option<String>,
}

#[derive(Default)]
struct RooflineLoopInfo {
    id: u128,
    pid: u32,
    tid: u32,
    file_name: u128,
    func_name: u128,
    line: u32,
    start: u64,
    bytes_load: u64,
    bytes_store: u64,
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
}

struct RooflineData {
    baseline_pid: i32,
    instrumented_pid: i32,
    loops: HashMap<u128, RooflineLoopInfo>,
    runs: Vec<(RooflineLoopInfo, u64)>,
    ops: Vec<RooflineLoopInfo>,
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
    timestamp: u64,
    module_address: u64,
}

impl RooflineData {
    fn new(info: &ScenarioInfo) -> Option<Self> {
        let ScenarioInfo::Roofline(info) = info else {
            return None;
        };
        Some(Self {
            baseline_pid: info.perf_pid,
            instrumented_pid: info.inst_pid,
            loops: HashMap::new(),
            runs: Vec::new(),
            ops: Vec::new(),
        })
    }

    fn consume(&mut self, event: &Event) -> Result<()> {
        match event.ty {
            EventType::RooflineLoopStart => {
                let location = event
                    .callstack
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("roofline loop start has no location"))?
                    .as_loc();
                self.loops.insert(
                    event.unique_id,
                    RooflineLoopInfo {
                        id: event.unique_id,
                        pid: event.process_id,
                        tid: event.thread_id,
                        file_name: location.file_name,
                        func_name: location.function_name,
                        line: location.line,
                        start: event.timestamp,
                        ..RooflineLoopInfo::default()
                    },
                );
            }
            EventType::RooflineLoopEnd => {
                let loop_info = self.loops.remove(&event.correlation_id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "roofline loop end references unknown loop {}",
                        event.correlation_id
                    )
                })?;
                if event.process_id as i32 == self.baseline_pid {
                    self.runs.push((loop_info, event.timestamp));
                } else if event.process_id as i32 == self.instrumented_pid {
                    self.ops.push(loop_info);
                }
            }
            EventType::RooflineBytesLoad => {
                self.loop_mut(event)?.bytes_load = event.value;
            }
            EventType::RooflineBytesStore => {
                self.loop_mut(event)?.bytes_store = event.value;
            }
            EventType::RooflineScalarIntOps => {
                self.loop_mut(event)?.scalar_int_ops = event.value;
            }
            EventType::RooflineScalarFloatOps => {
                self.loop_mut(event)?.scalar_float_ops = event.value;
            }
            EventType::RooflineScalarDoubleOps => {
                self.loop_mut(event)?.scalar_double_ops = event.value;
            }
            EventType::RooflineVectorIntOps => {
                self.loop_mut(event)?.vector_int_ops = event.value;
            }
            EventType::RooflineVectorFloatOps => {
                self.loop_mut(event)?.vector_float_ops = event.value;
            }
            EventType::RooflineVectorDoubleOps => {
                self.loop_mut(event)?.vector_double_ops = event.value;
            }
            _ => {}
        }
        Ok(())
    }

    fn loop_mut(&mut self, event: &Event) -> Result<&mut RooflineLoopInfo> {
        self.loops.get_mut(&event.parent_id).ok_or_else(|| {
            anyhow::anyhow!(
                "roofline event references unknown parent {}",
                event.parent_id
            )
        })
    }
}

async fn process_pmu_counters(
    connection: &sqlite::Connection,
    record_info: &RecordInfo,
    res_dir: &Path,
    pb: &mut kdam::Bar,
) -> Result<()> {
    let info = &record_info.scenario_info;
    let events = match info {
        ScenarioInfo::Snapshot(s) => &s.counters,
        ScenarioInfo::Mem(m) => &m.counters,
        ScenarioInfo::Roofline(r) => &r.counters,
        ScenarioInfo::TMA(t) => &t.counters,
    };

    let default_value = if matches!(info, ScenarioInfo::TMA(_)) {
        ""
    } else {
        " DEFAULT 0"
    };
    let mut seen_columns = HashSet::new();
    let event_columns = events
        .iter()
        .map(get_event_column_name)
        .filter(|column| column != "pmu_unknown")
        .filter(|column| seen_columns.insert(column.clone()))
        .collect::<Vec<_>>();
    let str_events = event_columns
        .iter()
        // NULL means this event was not a member of the sampled perf group;
        // zero means it was a member but observed no delta. TMA needs this
        // distinction to keep a formula inside its coherent group.
        .map(|column| format!("{} INTEGER{}", quote_identifier(column), default_value))
        .collect::<Vec<_>>()
        .join(", ");
    let event_schema = if str_events.is_empty() {
        String::new()
    } else {
        format!(", {str_events}")
    };

    connection.execute(format!(
        "
            CREATE TABLE pmu_counters (
                unique_id BINARY(128),
                process_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                cpu INTEGER,
                time_enabled INTEGER NOT NULL,
                time_running INTEGER NOT NULL,
                confidence REAL NOT NULL,
                timestamp INTEGER NOT NULL,
                ip INTEGER NOT NULL,
                call_stack TEXT{}
            );
            CREATE TABLE cpu_observations (
                process_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                cpu INTEGER,
                timestamp INTEGER NOT NULL,
                interval_start_ns INTEGER,
                weight_ns INTEGER NOT NULL,
                source TEXT NOT NULL,
                call_stack TEXT NOT NULL
            );
        ",
        event_schema
    ))?;

    let mut roofline = RooflineData::new(info);
    if roofline.is_some() {
        create_roofline_tables(connection)?;
    }

    let file = File::open(res_dir.join("events.bin"))
        .await
        .expect("failed to open events.bin");

    let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
    map.advise(Advice::Sequential)
        .expect("Failed to advice sequential reads");

    pb.reset(Some(map.len()));
    pb.write("Collecting hotspots")?;

    let strings_file = std::fs::File::open(res_dir.join("strings.json"))?;
    let strings: Vec<IString> = serde_json::from_reader(strings_file)?;
    let strings = strings
        .into_iter()
        .map(|string| (string.id, string.value))
        .collect::<HashMap<_, _>>();

    let proc_map_file = std::fs::File::open(res_dir.join("proc_map.json"))?;
    let proc_map: Vec<ProcMapEntry> = serde_json::from_reader(proc_map_file)?;

    let resolved_pm = utils::resolve_proc_maps(&proc_map);
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    let mut post_hoc_unwinder = crate::unwind::PostHocUnwinder::new(&proc_map);

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    let mut counters = HashMap::<String, u64>::with_capacity(event_columns.len());
    let mut lead_event: Option<CounterLead> = None;
    let mut folded_stack = String::new();

    let mut proc_map_stmt = connection.prepare(
        "INSERT INTO proc_map (ip, func_name, file_name, line, module_path) VALUES (?, ?, ?, ?, ?);",
    )?;

    let mut known_ips = HashSet::<u64>::new();
    let mut resolved_ips = HashMap::<(u32, u64), ResolvedIp>::new();

    // Core-cluster topology, used to attribute samples per core on
    // heterogeneous (big.LITTLE) systems. Empty on homogeneous hosts.
    let clusters: Vec<ClusterRanges> = record_info
        .cores
        .iter()
        .cloned()
        .map(|c| (c.family_id, c.name, parse_cpumask(&c.cpus)))
        .collect();
    let cpu_clock_source = record_info
        .cpu_clock_source
        .map(CpuClockSource::as_str)
        .unwrap_or("legacy_unknown");

    let mut flamegraph_cycles = HashMap::<String, u64>::new();
    let mut flamegraph_instructions = HashMap::<String, u64>::new();
    // family_id -> (display name, folded stack -> value)
    let mut per_core_cycles = HashMap::<String, (String, HashMap<String, u64>)>::new();
    let mut per_core_instructions = HashMap::<String, (String, HashMap<String, u64>)>::new();

    let insert_columns = event_columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_columns = if insert_columns.is_empty() {
        String::new()
    } else {
        format!(", {insert_columns}")
    };
    let placeholders = std::iter::repeat_n("?", 10 + event_columns.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut counter_stmt = connection.prepare(format!(
        "INSERT INTO pmu_counters (
            unique_id, process_id, thread_id, cpu, time_enabled, time_running,
            confidence, timestamp, ip, call_stack{insert_columns}
         ) VALUES ({placeholders});"
    ))?;
    let mut cpu_observation_stmt = connection.prepare(
        "INSERT INTO cpu_observations (
            process_id, thread_id, cpu, timestamp, interval_start_ns,
            weight_ns, source, call_stack
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
    )?;

    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;
    let result = (|| -> Result<()> {
        let mut next_progress = 1024 * 1024;
        while (cursor.position() as usize) < map.len() {
            #[cfg(all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ))]
            let mut evt = Event::read_binary(&mut cursor).expect("Failed to decode event");
            #[cfg(not(all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )))]
            let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");
            let position = cursor.position() as usize;
            if position >= next_progress {
                pb.update_to(position)?;
                next_progress = position.saturating_add(1024 * 1024);
            }

            if evt.ty.is_roofline() {
                if let Some(roofline) = &mut roofline {
                    roofline.consume(&evt)?;
                }
                continue;
            }

            if !evt.ty.is_pmu() && !evt.ty.is_os() {
                continue;
            }

            #[cfg(all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ))]
            post_hoc_unwinder.unwind_event(&mut evt);

            if !resolved_pm.has_process(evt.process_id) {
                continue;
            }

            let is_new_group = lead_event
                .as_ref()
                .is_none_or(|lead| evt.correlation_id != lead.correlation_id);
            if is_new_group {
                if let Some(lead) = &lead_event {
                    insert_cpu_observation(
                        &mut cpu_observation_stmt,
                        lead,
                        &counters,
                        cpu_clock_source,
                    )?;
                    insert_counter_group(
                        &mut counter_stmt,
                        lead,
                        &counters,
                        &event_columns,
                        matches!(info, ScenarioInfo::TMA(_)),
                    )?;
                    counters.clear();
                }

                folded_stack = resolve_folded_stack(
                    &resolved_pm,
                    &mut resolved_ips,
                    evt.process_id,
                    &evt.callstack,
                );

                for frame in &evt.callstack {
                    let CallFrame::IP(ip) = frame else {
                        continue;
                    };
                    if !known_ips.insert(*ip) {
                        continue;
                    }
                    let resolved = resolve_ip(&resolved_pm, &mut resolved_ips, evt.process_id, *ip);
                    proc_map_stmt.reset()?;
                    proc_map_stmt.bind((1, *ip as i64))?;
                    proc_map_stmt.bind((2, resolved.function.as_str()))?;
                    proc_map_stmt.bind((3, resolved.file.as_str()))?;
                    proc_map_stmt.bind((4, resolved.line as i64))?;
                    proc_map_stmt.bind((5, resolved.module_path.as_deref()))?;
                    proc_map_stmt.next()?;
                }

                lead_event = Some(CounterLead {
                    unique_id: evt.unique_id,
                    correlation_id: evt.correlation_id,
                    process_id: evt.process_id,
                    thread_id: evt.thread_id,
                    cpu: evt.cpu,
                    time_enabled: evt.time_enabled,
                    time_running: evt.time_running,
                    timestamp: evt.timestamp,
                    callstack: evt.callstack.clone(),
                });
            }

            // Frequency sampling makes every delivered overflow one observation.
            // Do not weight it by the cumulative counter delta: after a lost or
            // throttled interval that delta spans many seconds and cannot be
            // attributed to the single IP which happens to arrive next.
            // Zero is the initial KPC baseline and is not an actual observation.
            if evt.ty == EventType::PmuCycles && !folded_stack.is_empty() {
                if let Some(weight) = flamegraph_sample_weight(evt.value) {
                    *flamegraph_cycles.entry(folded_stack.clone()).or_default() += weight;
                    if let Some((family_id, name)) = cluster_of(&clusters, evt.cpu) {
                        *per_core_cycles
                            .entry(family_id.to_owned())
                            .or_insert_with(|| (name.to_owned(), HashMap::new()))
                            .1
                            .entry(folded_stack.clone())
                            .or_default() += weight;
                    }
                }
            } else if evt.ty == EventType::PmuInstructions
                && !folded_stack.is_empty()
                && let Some(weight) = flamegraph_sample_weight(evt.value)
            {
                *flamegraph_instructions
                    .entry(folded_stack.clone())
                    .or_default() += weight;
                if let Some((family_id, name)) = cluster_of(&clusters, evt.cpu) {
                    *per_core_instructions
                        .entry(family_id.to_owned())
                        .or_insert_with(|| (name.to_owned(), HashMap::new()))
                        .1
                        .entry(folded_stack.clone())
                        .or_default() += weight;
                }
            }

            let event_name = strings.get(&evt.name).cloned().unwrap_or_default();
            counters.insert(get_event_column_name(&(evt.ty, event_name)), evt.value);
        }

        if let Some(lead_event) = &lead_event {
            insert_cpu_observation(
                &mut cpu_observation_stmt,
                lead_event,
                &counters,
                cpu_clock_source,
            )?;
            insert_counter_group(
                &mut counter_stmt,
                lead_event,
                &counters,
                &event_columns,
                matches!(info, ScenarioInfo::TMA(_)),
            )?;
        }

        if let Some(roofline) = roofline.take() {
            persist_roofline_data(connection, roofline)?;
        }
        pb.update_to(map.len())?;
        Ok(())
    })();
    drop(cpu_observation_stmt);
    drop(counter_stmt);
    drop(proc_map_stmt);
    finish_transaction(connection, result)?;

    write_flamegraph(res_dir, "flamegraph_cycles", flamegraph_cycles).await?;
    write_flamegraph(res_dir, "flamegraph_instructions", flamegraph_instructions).await?;

    // Per-core flamegraphs on heterogeneous systems, e.g.
    // `flamegraph_cycles_cortex_a720.folded`.
    for (family_id, (_name, map)) in per_core_cycles {
        write_flamegraph(res_dir, &format!("flamegraph_cycles_{family_id}"), map).await?;
    }
    for (family_id, (_name, map)) in per_core_instructions {
        write_flamegraph(
            res_dir,
            &format!("flamegraph_instructions_{family_id}"),
            map,
        )
        .await?;
    }

    Ok(())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn resolve_ip<'a>(
    resolver: &symbolize::Resolver,
    cache: &'a mut HashMap<(u32, u64), ResolvedIp>,
    pid: u32,
    ip: u64,
) -> &'a ResolvedIp {
    cache.entry((pid, ip)).or_insert_with(|| {
        let frames = resolver.resolve(pid, ip);
        let functions = if frames.is_empty() {
            vec!["[unknown]".to_owned()]
        } else {
            frames.iter().map(|frame| frame.function.clone()).collect()
        };
        let primary = frames.first();
        let module_path = primary
            .and_then(|frame| frame.module.as_ref())
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| {
                resolver
                    .module_path(pid, ip)
                    .map(|path| path.to_string_lossy().into_owned())
            });
        ResolvedIp {
            functions,
            function: primary
                .map(|frame| frame.function.clone())
                .unwrap_or_else(|| "[unknown]".to_owned()),
            file: primary
                .and_then(|frame| frame.file.clone())
                .unwrap_or_else(|| "unknown".to_owned()),
            line: primary.and_then(|frame| frame.line).unwrap_or_default(),
            module_path,
        }
    })
}

fn resolve_folded_stack(
    resolver: &symbolize::Resolver,
    cache: &mut HashMap<(u32, u64), ResolvedIp>,
    pid: u32,
    callstack: &[CallFrame],
) -> String {
    let mut functions = SmallVec::<[String; 32]>::new();
    for frame in callstack.iter().rev() {
        match frame {
            CallFrame::Location(_) => functions.push("[instrumented]".to_owned()),
            CallFrame::IP(ip) => {
                let resolved = resolve_ip(resolver, cache, pid, *ip);
                functions.extend(resolved.functions.iter().rev().cloned());
            }
        }
    }
    functions.join(";")
}

fn create_roofline_tables(connection: &sqlite::Connection) -> Result<()> {
    connection.execute(
        "
        CREATE TABLE roofline_ops(
            unique_id BINARY(128), process_id INTEGER NOT NULL, thread_id INTEGER NOT NULL,
            file_name BINARY(128) NOT NULL, function_name BINARY(128) NOT NULL,
            line INTEGER NOT NULL, bytes_load INTEGER NOT NULL, bytes_store INTEGER NOT NULL,
            scalar_int_ops INTEGER NOT NULL, scalar_float_ops INTEGER NOT NULL,
            scalar_double_ops INTEGER NOT NULL, vector_int_ops INTEGER NOT NULL,
            vector_float_ops INTEGER NOT NULL, vector_double_ops INTEGER NOT NULL
        );
        CREATE TABLE roofline_loop_runs(
            unique_id BINARY(128), process_id INTEGER NOT NULL, thread_id INTEGER NOT NULL,
            file_name BINARY(128) NOT NULL, function_name BINARY(128) NOT NULL,
            line INTEGER NOT NULL, loop_start_ts INTEGER NOT NULL, loop_end_ts INTEGER NOT NULL
        );
        CREATE TABLE roofline_binary_loops(
            module_offset TEXT PRIMARY KEY,
            function_name TEXT NOT NULL,
            file_name TEXT NOT NULL,
            line INTEGER NOT NULL,
            trip_count INTEGER NOT NULL,
            duration_ns INTEGER,
            timing_samples INTEGER NOT NULL,
            timing_relative_error REAL,
            timing_quality TEXT NOT NULL,
            bytes_load INTEGER NOT NULL,
            bytes_store INTEGER NOT NULL,
            arch_bytes_load INTEGER NOT NULL,
            arch_bytes_store INTEGER NOT NULL,
            scalar_int_ops INTEGER NOT NULL,
            scalar_float_ops INTEGER NOT NULL,
            scalar_double_ops INTEGER NOT NULL,
            vector_int_ops INTEGER NOT NULL,
            vector_float_ops INTEGER NOT NULL,
            vector_double_ops INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

fn persist_roofline_data(connection: &sqlite::Connection, data: RooflineData) -> Result<()> {
    let mut run_stmt = connection.prepare(
        "INSERT INTO roofline_loop_runs (
            unique_id, process_id, thread_id, file_name, function_name, line,
            loop_start_ts, loop_end_ts
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
    )?;
    for (run, end) in data.runs {
        run_stmt.reset()?;
        bind_id(&mut run_stmt, 1, run.id)?;
        run_stmt.bind((2, run.pid as i64))?;
        run_stmt.bind((3, run.tid as i64))?;
        bind_id(&mut run_stmt, 4, run.file_name)?;
        bind_id(&mut run_stmt, 5, run.func_name)?;
        run_stmt.bind((6, run.line as i64))?;
        run_stmt.bind((7, run.start as i64))?;
        run_stmt.bind((8, end as i64))?;
        run_stmt.next()?;
    }

    let mut ops_stmt = connection.prepare(
        "INSERT INTO roofline_ops (
            unique_id, process_id, thread_id, file_name, function_name, line,
            bytes_load, bytes_store, scalar_int_ops, scalar_float_ops, scalar_double_ops,
            vector_int_ops, vector_float_ops, vector_double_ops
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
    )?;
    for ops in data.ops {
        ops_stmt.reset()?;
        bind_id(&mut ops_stmt, 1, ops.id)?;
        ops_stmt.bind((2, ops.pid as i64))?;
        ops_stmt.bind((3, ops.tid as i64))?;
        bind_id(&mut ops_stmt, 4, ops.file_name)?;
        bind_id(&mut ops_stmt, 5, ops.func_name)?;
        ops_stmt.bind((6, ops.line as i64))?;
        for (index, value) in [
            ops.bytes_load,
            ops.bytes_store,
            ops.scalar_int_ops,
            ops.scalar_float_ops,
            ops.scalar_double_ops,
            ops.vector_int_ops,
            ops.vector_float_ops,
            ops.vector_double_ops,
        ]
        .into_iter()
        .enumerate()
        {
            ops_stmt.bind((7 + index, value as i64))?;
        }
        ops_stmt.next()?;
    }
    Ok(())
}

fn process_binary_roofline_loops(
    connection: &sqlite::Connection,
    record_info: &RecordInfo,
    res_dir: &Path,
) -> Result<()> {
    use sqlite::State;

    let ScenarioInfo::Roofline(roofline_info) = &record_info.scenario_info else {
        return Ok(());
    };
    let Some(method) = roofline_info.method.as_deref() else {
        return Ok(());
    };
    if !matches!(method.accounting.as_str(), "qemu" | "dynamorio") || method.performance != "native"
    {
        return Ok(());
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

    let executable = Path::new(&artifact.executable);
    let object_data = std::fs::read(executable)
        .with_context(|| format!("read Roofline executable '{}'", executable.display()))?;
    let object = object::File::parse(object_data.as_slice())
        .with_context(|| format!("parse Roofline executable '{}'", executable.display()))?;
    let proc_maps: Vec<ProcMapEntry> =
        serde_json::from_reader(std::fs::File::open(res_dir.join("proc_map.json"))?)?;
    let mappings = executable_mappings(
        &proc_maps,
        roofline_info.perf_pid as u32,
        executable,
        &object,
    );
    if mappings.is_empty() {
        anyhow::bail!(
            "native Roofline samples have no executable mapping for '{}'",
            executable.display()
        );
    }

    let mut sample_statement = connection.prepare(
        "SELECT timestamp, ip FROM pmu_counters
         WHERE process_id = ? AND os_cpu_clock > 0 AND ip != 0
         ORDER BY timestamp;",
    )?;
    sample_statement.bind((1, roofline_info.perf_pid as i64))?;
    let mut samples = Vec::new();
    while sample_statement.next()? == State::Row {
        let timestamp = sample_statement.read::<i64, _>(0)? as u64;
        let ip = sample_statement.read::<i64, _>(1)? as u64;
        if let Some(module_address) = normalize_sample_ip(ip, &mappings) {
            samples.push(BinaryTimingSample {
                timestamp,
                module_address,
            });
        }
    }

    let sample_period_ns = 1_000_000_000_u64
        .checked_div(record_info.sampling_frequency_hz.unwrap_or(1_000).max(1))
        .unwrap_or(1_000_000);
    let mut insert = connection.prepare(
        "INSERT INTO roofline_binary_loops (
            module_offset, function_name, file_name, line, trip_count,
            duration_ns, timing_samples, timing_relative_error, timing_quality,
            bytes_load, bytes_store, arch_bytes_load, arch_bytes_store,
            scalar_int_ops, scalar_float_ops,
            scalar_double_ops, vector_int_ops, vector_float_ops, vector_double_ops
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
    )?;

    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;
    let result = (|| -> Result<()> {
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
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>();
            let timing_samples = timestamps.len() as u64;
            // Treat samples as independent occupancy observations. The 95%
            // relative sampling error is approximately 1.96/sqrt(N). Only
            // rows at or below 10% are allowed to produce a throughput point.
            let relative_error =
                (timing_samples > 0).then(|| 1.96 / (timing_samples as f64).sqrt());
            let timing_quality = match (counts_are_classified(&loop_info.inclusive), relative_error)
            {
                (false, _) => "unclassified-instructions",
                (true, Some(error)) if error <= 0.10 => "advisor-grade",
                (true, Some(error)) if error <= 0.20 => "low-confidence",
                _ => "insufficient-samples",
            };
            let duration_ns = (timing_quality == "advisor-grade")
                .then(|| sampled_active_duration(&timestamps, sample_period_ns));
            let counts = loop_info.inclusive;

            insert.reset()?;
            insert.bind((1, loop_info.module_offset.as_str()))?;
            insert.bind((2, loop_info.function.as_deref().unwrap_or("[unknown loop]")))?;
            insert.bind((3, loop_info.file.as_deref().unwrap_or("")))?;
            insert.bind((4, loop_info.line.unwrap_or_default() as i64))?;
            insert.bind((5, sqlite_u64(loop_info.trip_count)))?;
            insert.bind((6, duration_ns.map(sqlite_u64)))?;
            insert.bind((7, sqlite_u64(timing_samples)))?;
            insert.bind((8, relative_error))?;
            insert.bind((9, timing_quality))?;
            for (index, value) in [
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
            ]
            .into_iter()
            .enumerate()
            {
                insert.bind((10 + index, sqlite_u64(value)))?;
            }
            insert.next()?;
        }
        Ok(())
    })();
    drop(insert);
    finish_transaction(connection, result)
}

fn executable_mappings<'data>(
    proc_maps: &[ProcMapEntry],
    pid: u32,
    executable: &Path,
    object: &object::File<'data>,
) -> Vec<ModuleMapping> {
    proc_maps
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

fn sampled_active_duration(timestamps: &[u64], period_ns: u64) -> u64 {
    let mut intervals = timestamps
        .iter()
        .map(|timestamp| (timestamp.saturating_sub(period_ns), *timestamp))
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut duration = 0_u64;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in intervals {
        match current {
            Some((current_start, current_end)) if start <= current_end => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                duration = duration.saturating_add(current_end.saturating_sub(current_start));
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        duration = duration.saturating_add(end.saturating_sub(start));
    }
    duration
}

fn parse_hex_address(value: &str) -> Result<u64> {
    let value = value
        .strip_prefix("0x")
        .with_context(|| format!("binary loop address is not hexadecimal: '{value}'"))?;
    u64::from_str_radix(value, 16)
        .with_context(|| format!("invalid binary loop address '0x{value}'"))
}

fn sqlite_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn counts_are_classified(counts: &BinaryLoopCounts) -> bool {
    counts.unclassified_instructions == 0
}

#[cfg(test)]
mod binary_roofline_tests {
    use super::*;
    use sqlite::State;

    #[test]
    fn sampled_duration_unions_parallel_and_separated_observations() {
        assert_eq!(sampled_active_duration(&[1_000, 1_500], 1_000), 1_500);
        assert_eq!(sampled_active_duration(&[1_000, 3_000], 1_000), 2_000);
        assert_eq!(sampled_active_duration(&[], 1_000), 0);
    }

    #[test]
    fn requires_complete_operation_classification() {
        assert!(counts_are_classified(&BinaryLoopCounts::default()));
        assert!(!counts_are_classified(&BinaryLoopCounts {
            unclassified_instructions: 1,
            ..BinaryLoopCounts::default()
        }));
    }

    #[tokio::test]
    async fn binary_loops_replace_the_whole_process_placeholder() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE strings (id BLOB, string TEXT);
                 CREATE TABLE roofline_ops(
                    unique_id BLOB, process_id INTEGER, thread_id INTEGER,
                    file_name BLOB, function_name BLOB, line INTEGER,
                    bytes_load INTEGER, bytes_store INTEGER,
                    scalar_int_ops INTEGER, scalar_float_ops INTEGER,
                    scalar_double_ops INTEGER, vector_int_ops INTEGER,
                    vector_float_ops INTEGER, vector_double_ops INTEGER
                 );
                 CREATE TABLE roofline_loop_runs(
                    unique_id BLOB, process_id INTEGER, thread_id INTEGER,
                    file_name BLOB, function_name BLOB, line INTEGER,
                    loop_start_ts INTEGER, loop_end_ts INTEGER
                 );
                 CREATE TABLE roofline_binary_loops(
                    module_offset TEXT PRIMARY KEY, function_name TEXT, file_name TEXT,
                    line INTEGER, trip_count INTEGER, duration_ns INTEGER,
                    timing_samples INTEGER, timing_relative_error REAL, timing_quality TEXT,
                    bytes_load INTEGER, bytes_store INTEGER,
                    arch_bytes_load INTEGER, arch_bytes_store INTEGER, scalar_int_ops INTEGER,
                    scalar_float_ops INTEGER, scalar_double_ops INTEGER, vector_int_ops INTEGER,
                    vector_float_ops INTEGER, vector_double_ops INTEGER
                 );
                 INSERT INTO roofline_binary_loops VALUES (
                    '0x100', 'kernel', 'kernel.c', 7, 100, 1000000000,
                    400, 0.098, 'advisor-grade', 800, 200, 4000, 1000,
                    0, 0, 2000000000, 0, 0, 6000000000
                 );
                 -- A cache-resident loop: zero modeled DRAM traffic, so its
                 -- arithmetic intensity must come from architectural traffic
                 -- instead of dropping the loop off the chart entirely.
                 INSERT INTO roofline_binary_loops VALUES (
                    '0x200', 'resident', 'kernel.c', 20, 100, 1000000000,
                    400, 0.098, 'advisor-grade', 0, 0, 1000, 1000,
                    0, 0, 0, 0, 0, 8000000000
                 );",
            )
            .unwrap();
        create_roofline_view(&connection).await.unwrap();

        let mut statement = connection
            .prepare(
                "SELECT function_name, scalar_double_ops, vector_double_ops,
                        scalar_double_ai, vector_double_ai, timing_quality, module_offset
                 FROM roofline;",
            )
            .unwrap();
        assert_eq!(statement.next().unwrap(), State::Row);
        assert_eq!(statement.read::<String, _>(0).unwrap(), "kernel");
        assert_eq!(statement.read::<f64, _>(1).unwrap(), 2_000_000_000.0);
        assert_eq!(statement.read::<f64, _>(2).unwrap(), 6_000_000_000.0);
        // Arithmetic intensity is architectural (cache-aware roofline), so
        // this divides by 4000 + 1000 architectural bytes, not by the 800 +
        // 200 bytes that happened to reach DRAM.
        assert_eq!(statement.read::<f64, _>(3).unwrap(), 400_000.0);
        assert_eq!(statement.read::<f64, _>(4).unwrap(), 1_200_000.0);
        assert_eq!(statement.read::<String, _>(5).unwrap(), "advisor-grade");
        assert_eq!(statement.read::<String, _>(6).unwrap(), "0x100");

        // A cache-resident loop has zero DRAM traffic but the same kind of
        // finite architectural intensity: 8e9 ops / 2000 bytes.
        assert_eq!(statement.next().unwrap(), State::Row);
        assert_eq!(statement.read::<String, _>(0).unwrap(), "resident");
        assert_eq!(statement.read::<f64, _>(4).unwrap(), 4_000_000.0);
        assert_eq!(statement.next().unwrap(), State::Done);
    }
}

fn flamegraph_sample_weight(counter_delta: u64) -> Option<u64> {
    (counter_delta != 0).then_some(1)
}

fn insert_counter_group(
    statement: &mut sqlite::Statement<'_>,
    lead_event: &CounterLead,
    counters: &HashMap<String, u64>,
    event_columns: &[String],
    missing_is_null: bool,
) -> Result<()> {
    if !counter_group_has_profile_data(lead_event, counters) {
        return Ok(());
    }

    if !event_columns
        .iter()
        .any(|column| counters.contains_key(column))
    {
        return Ok(());
    }

    let confidence = if lead_event.time_enabled > 0 {
        lead_event.time_running as f64 / lead_event.time_enabled as f64
    } else {
        0.0
    };
    let call_stack = serialized_call_stack(&lead_event.callstack);
    statement.reset()?;
    bind_id(statement, 1, lead_event.unique_id)?;
    statement.bind((2, lead_event.process_id as i64))?;
    statement.bind((3, lead_event.thread_id as i64))?;
    statement.bind((
        4,
        (lead_event.cpu != u32::MAX).then_some(lead_event.cpu as i64),
    ))?;
    statement.bind((5, lead_event.time_enabled as i64))?;
    statement.bind((6, lead_event.time_running as i64))?;
    statement.bind((7, confidence))?;
    statement.bind((8, lead_event.timestamp as i64))?;
    statement.bind((
        9,
        lead_event.callstack.first().map(|f| f.as_ip()).unwrap_or(0) as i64,
    ))?;
    statement.bind((10, call_stack.as_str()))?;
    for (offset, column) in event_columns.iter().enumerate() {
        let value = counters
            .get(column)
            .copied()
            .map(|value| value as i64)
            .or_else(|| (!missing_is_null).then_some(0));
        statement.bind((11 + offset, value))?;
    }
    statement.next()?;
    Ok(())
}

fn bind_id(statement: &mut sqlite::Statement<'_>, index: usize, id: u128) -> sqlite::Result<()> {
    let bytes = id.to_be_bytes();
    statement.bind((index, bytes.as_slice()))
}

fn insert_cpu_observation(
    statement: &mut sqlite::Statement<'_>,
    lead_event: &CounterLead,
    counters: &HashMap<String, u64>,
    source: &str,
) -> Result<()> {
    let Some(weight_ns) = counters
        .get("os_cpu_clock")
        .copied()
        .filter(|value| *value > 0)
    else {
        return Ok(());
    };

    let call_stack = serialized_call_stack(&lead_event.callstack);
    statement.reset()?;
    statement.bind((1, lead_event.process_id as i64))?;
    statement.bind((2, lead_event.thread_id as i64))?;
    statement.bind((
        3,
        (lead_event.cpu != u32::MAX).then_some(lead_event.cpu as i64),
    ))?;
    statement.bind((4, lead_event.timestamp as i64))?;
    let interval_start_ns = (source == CpuClockSource::CounterDelta.as_str()
        && lead_event.time_enabled > 0)
        .then_some(lead_event.timestamp.saturating_sub(lead_event.time_enabled) as i64);
    statement.bind((5, interval_start_ns))?;
    statement.bind((6, weight_ns as i64))?;
    statement.bind((7, source))?;
    statement.bind((8, call_stack.as_str()))?;
    statement.next()?;
    Ok(())
}

fn serialized_call_stack(callstack: &[CallFrame]) -> String {
    format!(
        "[{}]",
        callstack
            .iter()
            .map(|frame| frame.as_ip().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Persist one-second metric intervals and a machine-readable dominant verdict.
/// Keeping this as tables (rather than only a view) makes the data available to
/// the summary UI and exporters without re-running formula expansion.
fn create_tma_intervals_and_summary(
    connection: &sqlite::Connection,
    info: &mperf_data::TMAInfo,
) -> Result<()> {
    connection.execute("CREATE TABLE tma_intervals (start_ns INTEGER NOT NULL, metric TEXT NOT NULL, value REAL);
                        CREATE TABLE tma_summary (metric TEXT PRIMARY KEY, value REAL, verdict TEXT);")?;
    for metric in &info.metrics {
        let expression = pmu_data::arith_parser::try_parse_expr(&metric.formula)
            .map_err(|error| anyhow::anyhow!("invalid TMA formula '{}': {error}", metric.name))?;
        let marker = metric
            .group
            .as_ref()
            .and_then(|name| {
                info.groups
                    .iter()
                    .find(|group| &group.name == name)
                    .and_then(|group| {
                        group.events.iter().find(|event| {
                            event.as_str() != "cycles" && event.as_str() != "instructions"
                        })
                    })
            })
            .map(|event| tma_marker_column(&info.counters, event));
        let sql = build_tma_sql_expr(
            &info.metrics,
            &info.counters,
            &info.constants,
            &expression,
            marker.as_deref(),
        );
        let escaped = metric.name.replace('\'', "''");
        connection.execute(format!(
            "INSERT INTO tma_intervals (start_ns, metric, value)
             SELECT (timestamp / 1000000000) * 1000000000, '{escaped}', {sql}
             FROM pmu_counters GROUP BY timestamp / 1000000000;"
        ))?;
        connection.execute(format!(
            "INSERT INTO tma_summary (metric, value)
             SELECT '{escaped}', {sql} FROM pmu_counters;"
        ))?;
    }
    connection.execute(
        "UPDATE tma_summary SET verdict = 'dominant' WHERE metric =
         (SELECT metric FROM tma_summary WHERE value IS NOT NULL ORDER BY value DESC LIMIT 1);",
    )?;
    Ok(())
}

fn counter_group_has_profile_data(
    lead_event: &CounterLead,
    counters: &HashMap<String, u64>,
) -> bool {
    !lead_event.callstack.is_empty() && counters.values().any(|value| *value != 0)
}

#[cfg(test)]
mod counter_group_tests {
    use super::{
        CounterLead, counter_group_has_profile_data, insert_counter_group, insert_cpu_observation,
    };
    use mperf_data::CallFrame;
    use smallvec::SmallVec;
    use std::collections::HashMap;

    fn event(callstack: SmallVec<[CallFrame; 32]>) -> CounterLead {
        CounterLead {
            unique_id: 1,
            correlation_id: 1,
            thread_id: 1,
            process_id: 1,
            cpu: 0,
            time_enabled: 1,
            time_running: 1,
            timestamp: 1,
            callstack,
        }
    }

    #[test]
    fn rejects_empty_stacks_and_all_zero_groups() {
        let mut counters = HashMap::from([("pmu_cycles".to_string(), 1)]);
        assert!(!counter_group_has_profile_data(
            &event(SmallVec::new()),
            &counters
        ));

        counters.insert("pmu_cycles".to_string(), 0);
        assert!(!counter_group_has_profile_data(
            &event(SmallVec::from_slice(&[CallFrame::IP(1)])),
            &counters
        ));

        counters.insert("pmu_cycles".to_string(), 1);
        assert!(counter_group_has_profile_data(
            &event(SmallVec::from_slice(&[CallFrame::IP(1)])),
            &counters
        ));
    }

    #[test]
    fn persists_logical_cpu_and_uses_null_for_unknown_cpu() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE pmu_counters (
                    unique_id BINARY(128),
                    process_id INTEGER NOT NULL,
                    thread_id INTEGER NOT NULL,
                    cpu INTEGER,
                    time_enabled INTEGER NOT NULL,
                    time_running INTEGER NOT NULL,
                    confidence REAL NOT NULL,
                    timestamp INTEGER NOT NULL,
                    ip INTEGER NOT NULL,
                    call_stack TEXT,
                    pmu_cycles INTEGER
                );",
            )
            .unwrap();
        let mut statement = connection
            .prepare(
                "INSERT INTO pmu_counters (
                    unique_id, process_id, thread_id, cpu, time_enabled, time_running,
                    confidence, timestamp, ip, call_stack, pmu_cycles
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
            )
            .unwrap();
        let counters = HashMap::from([("pmu_cycles".to_string(), 1)]);
        let columns = vec!["pmu_cycles".to_string()];

        let mut known_cpu = event(SmallVec::from_slice(&[CallFrame::IP(1)]));
        known_cpu.cpu = 7;
        insert_counter_group(&mut statement, &known_cpu, &counters, &columns, false).unwrap();

        let mut unknown_cpu = event(SmallVec::from_slice(&[CallFrame::IP(2)]));
        unknown_cpu.cpu = u32::MAX;
        insert_counter_group(&mut statement, &unknown_cpu, &counters, &columns, false).unwrap();
        drop(statement);

        let cpus = connection
            .prepare("SELECT cpu FROM pmu_counters ORDER BY ip;")
            .unwrap()
            .into_iter()
            .map(|row| row.unwrap().try_read::<Option<i64>, _>("cpu").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cpus, vec![Some(7), None]);
    }

    #[test]
    fn preserves_cpu_observations_when_stack_collection_fails() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE cpu_observations (
                    process_id INTEGER NOT NULL,
                    thread_id INTEGER NOT NULL,
                    cpu INTEGER,
                    timestamp INTEGER NOT NULL,
                    interval_start_ns INTEGER,
                    weight_ns INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    call_stack TEXT NOT NULL
                );",
            )
            .unwrap();
        let mut statement = connection
            .prepare(
                "INSERT INTO cpu_observations (
                    process_id, thread_id, cpu, timestamp, interval_start_ns,
                    weight_ns, source, call_stack
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
            )
            .unwrap();
        let lead = event(SmallVec::new());
        let counters = HashMap::from([("os_cpu_clock".to_string(), 1_000)]);

        insert_cpu_observation(&mut statement, &lead, &counters, "sampled_occupancy").unwrap();
        drop(statement);

        let row = connection
            .prepare(
                "SELECT cpu, interval_start_ns, weight_ns, source, call_stack
                 FROM cpu_observations LIMIT 1;",
            )
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(row.try_read::<i64, _>("cpu").unwrap(), 0);
        assert_eq!(row.try_read::<i64, _>("weight_ns").unwrap(), 1_000);
        assert_eq!(
            row.try_read::<&str, _>("source").unwrap(),
            "sampled_occupancy"
        );
        assert_eq!(
            row.try_read::<Option<i64>, _>("interval_start_ns").unwrap(),
            None
        );
        assert_eq!(row.try_read::<&str, _>("call_stack").unwrap(), "[]");
    }

    #[test]
    fn counter_delta_persists_its_first_wall_time_interval() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE cpu_observations (
                    process_id INTEGER NOT NULL,
                    thread_id INTEGER NOT NULL,
                    cpu INTEGER,
                    timestamp INTEGER NOT NULL,
                    interval_start_ns INTEGER,
                    weight_ns INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    call_stack TEXT NOT NULL
                );",
            )
            .unwrap();
        let mut statement = connection
            .prepare(
                "INSERT INTO cpu_observations (
                    process_id, thread_id, cpu, timestamp, interval_start_ns,
                    weight_ns, source, call_stack
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
            )
            .unwrap();
        let mut lead = event(SmallVec::new());
        lead.timestamp = 10_000_000;
        lead.time_enabled = 10_000_000;
        let counters = HashMap::from([("os_cpu_clock".to_string(), 2_000_000)]);

        insert_cpu_observation(&mut statement, &lead, &counters, "counter_delta").unwrap();
        drop(statement);

        let row = connection
            .prepare(
                "SELECT timestamp, interval_start_ns, weight_ns
                 FROM cpu_observations LIMIT 1;",
            )
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(row.try_read::<i64, _>("timestamp").unwrap(), 10_000_000);
        assert_eq!(row.try_read::<i64, _>("interval_start_ns").unwrap(), 0);
        assert_eq!(row.try_read::<i64, _>("weight_ns").unwrap(), 2_000_000);
    }
}

/// Parse a sysfs cpumask list such as `"0,5-11"` into inclusive `(start, end)`
/// ranges.
fn parse_cpumask(mask: &str) -> Vec<(u32, u32)> {
    mask.trim()
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            match part.split_once('-') {
                Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
                None => {
                    let v: u32 = part.parse().ok()?;
                    Some((v, v))
                }
            }
        })
        .collect()
}

/// Find the `(family_id, name)` of the core cluster a CPU belongs to.
fn cluster_of(clusters: &[ClusterRanges], cpu: u32) -> Option<(&str, &str)> {
    if cpu == u32::MAX {
        return None;
    }
    clusters
        .iter()
        .find(|(_, _, ranges)| ranges.iter().any(|(a, b)| cpu >= *a && cpu <= *b))
        .map(|(family_id, name, _)| (family_id.as_str(), name.as_str()))
}

/// Write a folded stack collapse map to `<stem>.folded` and, when the map is
/// non-empty, render it to `<stem>.svg`.
async fn write_flamegraph(res_dir: &Path, stem: &str, map: HashMap<String, u64>) -> Result<()> {
    let lines = map
        .into_iter()
        .map(|(key, value)| format!("{} {}", key, value))
        .collect::<Vec<_>>();

    let mut folded = File::create(res_dir.join(format!("{stem}.folded"))).await?;
    for line in &lines {
        folded.write_all(line.as_bytes()).await?;
        folded.write_all(b"\n").await?;
    }

    // Some counters can legitimately have no positive samples (in particular
    // for short-lived processes or unavailable hardware events). Inferno treats
    // an empty input as an error, but that must not invalidate the recording or
    // prevent other counters from being persisted.
    if lines.is_empty() {
        return Ok(());
    }

    let mut options = inferno::flamegraph::Options::default();
    options.reverse_stack_order = false;
    let svg = std::fs::File::create(res_dir.join(format!("{stem}.svg")))?;
    inferno::flamegraph::from_lines(&mut options, lines.iter().map(|s| s.as_str()), &svg)?;

    Ok(())
}

#[cfg(test)]
mod flamegraph_output_tests {
    use super::{flamegraph_sample_weight, write_flamegraph};
    use std::collections::HashMap;

    #[tokio::test]
    async fn empty_flamegraph_does_not_fail_postprocessing() {
        let dir =
            std::env::temp_dir().join(format!("mperf-empty-flamegraph-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();

        write_flamegraph(&dir, "empty", HashMap::new())
            .await
            .unwrap();

        assert_eq!(std::fs::read(dir.join("empty.folded")).unwrap(), b"");
        assert!(!dir.join("empty.svg").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cumulative_gap_does_not_dominate_flamegraph_weight() {
        assert_eq!(flamegraph_sample_weight(60_000_000_000), Some(1));
        assert_eq!(flamegraph_sample_weight(1), Some(1));
        assert_eq!(flamegraph_sample_weight(0), None);
    }
}

#[cfg(test)]
mod replay_benchmark {
    use super::perform_postprocessing;
    use std::time::Instant;

    #[tokio::test]
    #[ignore = "release replay benchmark; set MPERF_BENCH_RECORDING to a raw result directory"]
    async fn benchmark_recording_replay() {
        let source = std::env::var_os("MPERF_BENCH_RECORDING")
            .map(std::path::PathBuf::from)
            .expect("MPERF_BENCH_RECORDING must point to a recording");
        let runs = std::env::var("MPERF_BENCH_RUNS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(5)
            .max(1);
        let keep_results = std::env::var_os("MPERF_BENCH_KEEP").is_some();
        let mut durations = Vec::new();
        for iteration in 0..runs {
            let destination = std::env::temp_dir().join(format!(
                "mperf-postprocess-bench-{}-{iteration}",
                uuid::Uuid::now_v7()
            ));
            std::fs::create_dir(&destination).unwrap();
            for name in ["events.bin", "info.json", "proc_map.json", "strings.json"] {
                std::fs::copy(source.join(name), destination.join(name)).unwrap();
            }

            let started = Instant::now();
            perform_postprocessing(&destination, kdam::Bar::new(100))
                .await
                .unwrap();
            let elapsed = started.elapsed();
            let database_bytes = std::fs::metadata(destination.join("perf.db"))
                .unwrap()
                .len();
            eprintln!(
                "postprocess replay {}: {:.6}s, perf.db={} bytes",
                iteration + 1,
                elapsed.as_secs_f64(),
                database_bytes
            );
            durations.push(elapsed);
            if keep_results {
                eprintln!("kept replay at {}", destination.display());
            } else {
                std::fs::remove_dir_all(&destination).unwrap();
            }
        }
        durations.sort_unstable();
        eprintln!(
            "median: {:.6}s",
            durations[durations.len() / 2].as_secs_f64()
        );
    }
}

async fn process_disassembly(
    connection: &sqlite::Connection,
    res_dir: &Path,
    pb: &mut kdam::Bar,
) -> Result<()> {
    use sqlite::State;

    populate_assembly_samples(connection)?;

    connection.execute(
        "
        CREATE TABLE IF NOT EXISTS assembly_lines (
            module_path TEXT NOT NULL,
            symbol TEXT,
            rel_address INTEGER NOT NULL,
            runtime_address INTEGER NOT NULL,
            instruction TEXT NOT NULL,
            source_file TEXT,
            source_line INTEGER,
            PRIMARY KEY (module_path, runtime_address)
        );
        ",
    )?;
    connection.execute(
        "CREATE TABLE IF NOT EXISTS assembly_module_metadata (
            module_path TEXT PRIMARY KEY,
            load_bias INTEGER NOT NULL
        );",
    )?;

    let proc_map_file = std::fs::File::open(res_dir.join("proc_map.json"))?;
    let proc_map: Vec<ProcMapEntry> = serde_json::from_reader(proc_map_file)?;

    let mut module_bias = HashMap::<String, i64>::new();
    for entry in proc_map {
        let load_bias = entry.address as i64 - entry.offset as i64;
        module_bias
            .entry(entry.filename.clone())
            .and_modify(|bias| {
                if load_bias < *bias {
                    *bias = load_bias;
                }
            })
            .or_insert(load_bias);
    }

    let disassembler = match default_disassembler() {
        Ok(disassembler) => disassembler,
        Err(err) => {
            eprintln!("skipping assembly extraction: {err}");
            return Ok(());
        }
    };

    let mut module_stmt = connection.prepare(
        "SELECT module_path, address FROM assembly_samples ORDER BY module_path, address;",
    )?;
    let mut sampled_addresses = HashMap::<String, Vec<u64>>::new();
    while let State::Row = module_stmt.next()? {
        sampled_addresses
            .entry(module_stmt.read::<String, _>(0)?)
            .or_default()
            .push(module_stmt.read::<i64, _>(1)? as u64);
    }
    let mut modules = sampled_addresses.into_iter().collect::<Vec<_>>();
    modules.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    if modules.is_empty() {
        create_assembly_stats_view(connection)?;
        return Ok(());
    }

    pb.reset(Some(modules.len()));
    pb.write("Extracting assembly")?;

    let mut insert_stmt = connection.prepare(
        "INSERT OR IGNORE INTO assembly_lines (module_path, symbol, rel_address, runtime_address, instruction, source_file, source_line)
         VALUES (?, ?, ?, ?, ?, ?, ?);",
    )?;
    let mut metadata_stmt = connection.prepare(
        "INSERT INTO assembly_module_metadata (module_path, load_bias)
         VALUES (?, ?)
         ON CONFLICT(module_path) DO UPDATE SET load_bias = excluded.load_bias;",
    )?;

    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;
    let result = (|| -> Result<()> {
        for (idx, (module_path, addresses)) in modules.iter().enumerate() {
            pb.update_to(idx + 1)?;
            let module_file = Path::new(module_path);
            if !module_file.exists() {
                continue;
            }

            let load_bias = module_bias.get(module_path).copied().unwrap_or(0);
            metadata_stmt.reset()?;
            metadata_stmt.bind((1, module_path.as_str()))?;
            metadata_stmt.bind((2, load_bias))?;
            metadata_stmt.next()?;

            let (targets, address_base) =
                sampled_disassembly_targets(module_file, load_bias, addresses)?;
            let request = DisassembleRequest {
                module_path: module_file.to_path_buf(),
                load_bias,
                targets,
            };
            let lines = match disassembler.disassemble(&request) {
                Ok(lines) => lines,
                Err(err) => {
                    eprintln!("failed to disassemble {}: {err}", module_path);
                    continue;
                }
            };

            for line in lines {
                let rel_address = line.rel_address.saturating_sub(address_base);
                let Some(runtime_address) = apply_load_bias(rel_address, load_bias) else {
                    continue;
                };
                insert_stmt.reset()?;
                insert_stmt.bind((1, module_path.as_str()))?;
                insert_stmt.bind((2, line.symbol.as_deref()))?;
                insert_stmt.bind((3, rel_address as i64))?;
                insert_stmt.bind((4, runtime_address as i64))?;
                insert_stmt.bind((5, line.instruction.as_str()))?;
                // Source annotations are intentionally omitted from the eager path;
                // loading full DWARF line tables dominates targeted disassembly.
                insert_stmt.bind((6, ()))?;
                insert_stmt.bind((7, ()))?;
                insert_stmt.next()?;
            }
        }
        Ok(())
    })();
    drop(insert_stmt);
    drop(metadata_stmt);
    finish_transaction(connection, result)?;

    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_assembly_module_rel_address
         ON assembly_lines(module_path, rel_address);",
    )?;
    create_assembly_stats_view(connection)?;

    Ok(())
}

fn populate_assembly_samples(connection: &sqlite::Connection) -> Result<()> {
    connection.execute(
        "
        CREATE TABLE IF NOT EXISTS assembly_samples (
            module_path TEXT NOT NULL,
            func_name TEXT NOT NULL,
            address INTEGER NOT NULL,
            samples INTEGER NOT NULL,
            cycles INTEGER NOT NULL,
            instructions INTEGER NOT NULL,
            branch_misses INTEGER NOT NULL,
            branch_instructions INTEGER NOT NULL,
            llc_misses INTEGER NOT NULL,
            llc_references INTEGER NOT NULL,
            PRIMARY KEY (module_path, func_name, address)
        );
        ",
    )?;

    let available_columns = connection
        .prepare("PRAGMA table_info(pmu_counters);")?
        .into_iter()
        .filter_map(|row| row.ok().map(|row| row.read::<&str, _>("name").to_owned()))
        .collect::<HashSet<_>>();
    let metric = |column: &str| {
        if available_columns.contains(column) {
            format!("COALESCE(p.{}, 0)", quote_identifier(column))
        } else {
            "0".to_owned()
        }
    };
    connection.execute("CREATE INDEX IF NOT EXISTS idx_proc_map_ip ON proc_map(ip);")?;
    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;
    let result = connection
        .execute(format!(
            "DELETE FROM assembly_samples;
             INSERT INTO assembly_samples (
                module_path, func_name, address, samples, cycles, instructions,
                branch_misses, branch_instructions, llc_misses, llc_references
             )
             SELECT
                m.module_path,
                COALESCE(m.func_name, '[unknown]'),
                p.ip,
                COUNT(*),
                SUM({}), SUM({}), SUM({}), SUM({}), SUM({}), SUM({})
             FROM pmu_counters p
             INNER JOIN proc_map m ON m.ip = p.ip
             WHERE m.module_path IS NOT NULL AND m.module_path <> ''
             GROUP BY m.module_path, COALESCE(m.func_name, '[unknown]'), p.ip;",
            metric("pmu_cycles"),
            metric("pmu_instructions"),
            metric("pmu_branch_misses"),
            metric("pmu_branch_instructions"),
            metric("pmu_llc_misses"),
            metric("pmu_llc_references"),
        ))
        .map_err(Into::into);
    finish_transaction(connection, result)?;

    Ok(())
}

fn create_assembly_stats_view(connection: &sqlite::Connection) -> Result<()> {
    connection.execute(
        "DROP VIEW IF EXISTS assembly_address_stats;
         CREATE VIEW assembly_address_stats AS
         SELECT module_path, func_name, address,
                SUM(samples) AS samples, SUM(cycles) AS cycles,
                SUM(instructions) AS instructions,
                SUM(branch_misses) AS branch_misses,
                SUM(branch_instructions) AS branch_instructions,
                SUM(llc_misses) AS llc_misses,
                SUM(llc_references) AS llc_references
         FROM assembly_samples
         GROUP BY module_path, func_name, address;",
    )?;
    Ok(())
}

#[derive(Clone)]
struct ObjectTextSymbol {
    start: u64,
    end: u64,
    raw_name: String,
    display_name: String,
}

fn sampled_disassembly_targets(
    module_path: &Path,
    load_bias: i64,
    runtime_addresses: &[u64],
) -> Result<(Vec<DisassembleTarget>, u64)> {
    let bytes = std::fs::read(module_path)?;
    let object = object::File::parse(bytes.as_slice())?;
    let mut symbols = object
        .symbols()
        .chain(object.dynamic_symbols())
        .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.address() != 0)
        .filter_map(|symbol| {
            let raw_name = symbol.name().ok()?.to_owned();
            Some((symbol.address(), symbol.size(), raw_name))
        })
        .collect::<Vec<_>>();
    symbols.sort_unstable_by_key(|symbol| symbol.0);
    symbols.dedup_by(|left, right| left.0 == right.0 && left.2 == right.2);
    let address_base = if load_bias > 0
        && symbols
            .first()
            .is_some_and(|symbol| symbol.0 >= load_bias as u64)
    {
        load_bias as u64
    } else {
        0
    };

    let mut text_symbols = Vec::with_capacity(symbols.len());
    for (index, (start, size, raw_name)) in symbols.iter().enumerate() {
        let next_start = symbols
            .iter()
            .skip(index + 1)
            .find_map(|candidate| (candidate.0 > *start).then_some(candidate.0))
            .unwrap_or(u64::MAX);
        let end = if *size > 0 {
            start.saturating_add(*size)
        } else {
            next_start
        };
        text_symbols.push(ObjectTextSymbol {
            start: start.saturating_sub(address_base),
            end: end.saturating_sub(address_base),
            raw_name: raw_name.clone(),
            display_name: addr2line::demangle_auto(Cow::Borrowed(raw_name), None).into_owned(),
        });
    }

    let mut selected = HashMap::<(u64, String), ObjectTextSymbol>::new();
    let mut fallback = Vec::<(u64, u64)>::new();
    for runtime_address in runtime_addresses.iter().copied() {
        let Some(relative) = remove_load_bias(runtime_address, load_bias) else {
            continue;
        };
        let insertion = text_symbols.partition_point(|symbol| symbol.start <= relative);
        let symbol = text_symbols[..insertion]
            .iter()
            .rev()
            .find(|symbol| relative < symbol.end);
        if let Some(symbol) = symbol {
            selected.insert((symbol.start, symbol.raw_name.clone()), symbol.clone());
        } else {
            fallback.push((relative.saturating_sub(256), relative.saturating_add(257)));
        }
    }

    let mut targets = selected
        .into_values()
        .map(|symbol| DisassembleTarget {
            raw_symbol: Some(symbol.raw_name),
            owner_symbol: symbol.display_name,
            start_address: symbol.start.saturating_add(address_base),
            end_address: symbol.end.saturating_add(address_base),
        })
        .collect::<Vec<_>>();
    fallback.sort_unstable();
    let mut merged = Vec::<(u64, u64)>::new();
    for (start, end) in fallback {
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    targets.extend(merged.into_iter().map(|(start, end)| DisassembleTarget {
        raw_symbol: None,
        owner_symbol: format!("[sampled@0x{start:x}]"),
        start_address: start.saturating_add(address_base),
        end_address: end.saturating_add(address_base),
    }));
    targets.sort_unstable_by(|left, right| {
        left.raw_symbol
            .is_none()
            .cmp(&right.raw_symbol.is_none())
            .then_with(|| {
                left.start_address
                    .cmp(&right.start_address)
                    .then_with(|| left.owner_symbol.cmp(&right.owner_symbol))
            })
    });
    Ok((targets, address_base))
}

fn apply_load_bias(relative: u64, load_bias: i64) -> Option<u64> {
    if load_bias >= 0 {
        relative.checked_add(load_bias as u64)
    } else {
        relative.checked_sub(load_bias.unsigned_abs())
    }
}

fn remove_load_bias(runtime: u64, load_bias: i64) -> Option<u64> {
    if load_bias >= 0 {
        runtime.checked_sub(load_bias as u64)
    } else {
        runtime.checked_add(load_bias.unsigned_abs())
    }
}

#[cfg(test)]
mod optimized_postprocessing_tests {
    use super::{RooflineData, bind_id, populate_assembly_samples, sampled_disassembly_targets};
    use mperf_data::{CallFrame, Event, EventType, Location, RooflineInfo, ScenarioInfo};
    use object::{Object, ObjectSymbol, SymbolKind};
    use sqlite::State;

    #[test]
    fn assembly_samples_are_aggregated_in_one_sql_pass() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE proc_map (
                    ip INTEGER, func_name TEXT, file_name TEXT, line INTEGER, module_path TEXT
                 );
                 CREATE TABLE pmu_counters (
                    ip INTEGER NOT NULL, pmu_cycles INTEGER, pmu_instructions INTEGER
                 );
                 INSERT INTO proc_map VALUES (4096, 'hot', 'hot.c', 1, '/tmp/hot');
                 INSERT INTO pmu_counters VALUES (4096, 10, 20), (4096, 30, 40);",
            )
            .unwrap();

        populate_assembly_samples(&connection).unwrap();
        let mut statement = connection
            .prepare("SELECT * FROM assembly_samples")
            .unwrap();
        assert_eq!(statement.next().unwrap(), State::Row);
        assert_eq!(statement.read::<i64, _>("samples").unwrap(), 2);
        assert_eq!(statement.read::<i64, _>("cycles").unwrap(), 40);
        assert_eq!(statement.read::<i64, _>("instructions").unwrap(), 60);
        assert_eq!(statement.read::<i64, _>("branch_misses").unwrap(), 0);
        assert_eq!(statement.next().unwrap(), State::Done);
    }

    #[test]
    fn sqlite_ids_preserve_full_u128_precision() {
        let connection = sqlite::open(":memory:").unwrap();
        connection.execute("CREATE TABLE ids (id BLOB);").unwrap();
        let mut insert = connection.prepare("INSERT INTO ids VALUES (?);").unwrap();
        for id in [u128::MAX - 1, u128::MAX] {
            insert.reset().unwrap();
            bind_id(&mut insert, 1, id).unwrap();
            insert.next().unwrap();
        }

        let mut count = connection
            .prepare("SELECT COUNT(DISTINCT id) FROM ids;")
            .unwrap();
        assert_eq!(count.next().unwrap(), State::Row);
        assert_eq!(count.read::<i64, _>(0).unwrap(), 2);
    }

    #[test]
    fn sampled_symbol_selection_avoids_unrelated_object_code() {
        let executable = std::env::current_exe().unwrap();
        let bytes = std::fs::read(&executable).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = object
            .symbols()
            .find(|symbol| {
                symbol.kind() == SymbolKind::Text && symbol.address() != 0 && symbol.name().is_ok()
            })
            .unwrap();
        let sampled_address = symbol.address() + 1;

        let (targets, _) = sampled_disassembly_targets(&executable, 0, &[sampled_address]).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(targets[0].raw_symbol.is_some());
        assert!(targets[0].start_address <= sampled_address);
        assert!(targets[0].end_address > sampled_address);
    }

    #[test]
    fn roofline_events_are_collected_during_the_pmu_pass() {
        let info = ScenarioInfo::Roofline(RooflineInfo {
            backend: "compiler".to_string(),
            perf_pid: 10,
            counters: Vec::new(),
            inst_pid: 20,
            method: None,
        });
        let mut data = RooflineData::new(&info).unwrap();
        let mut start = event(EventType::RooflineLoopStart, 10);
        start.unique_id = 7;
        start.callstack.push(CallFrame::Location(Location {
            function_name: 2,
            file_name: 1,
            line: 3,
        }));
        data.consume(&start).unwrap();

        let mut bytes = event(EventType::RooflineBytesLoad, 10);
        bytes.parent_id = 7;
        bytes.value = 64;
        data.consume(&bytes).unwrap();

        let mut end = event(EventType::RooflineLoopEnd, 10);
        end.correlation_id = 7;
        end.timestamp = 99;
        data.consume(&end).unwrap();

        assert_eq!(data.runs.len(), 1);
        assert_eq!(data.runs[0].0.bytes_load, 64);
        assert_eq!(data.runs[0].1, 99);
    }

    fn event(ty: EventType, process_id: u32) -> Event {
        Event {
            unique_id: 1,
            correlation_id: 1,
            parent_id: 0,
            ty,
            thread_id: 1,
            process_id,
            cpu: 0,
            time_enabled: 0,
            time_running: 0,
            value: 0,
            timestamp: 1,
            name: 0,
            callstack: smallvec::SmallVec::new(),
            user_regs: None,
            user_stack: Vec::new(),
        }
    }
}

async fn create_hotspots_view(connection: &sqlite::Connection) -> Result<()> {
    connection.execute("
    CREATE VIEW hotspots
    AS
    SELECT
        proc_map.func_name as func_name,
        (SUM(pmu_counters.pmu_cycles) * 1.0 / (SELECT SUM(pmu_cycles) FROM pmu_counters)) AS total,
        SUM(pmu_counters.pmu_cycles) AS cycles,
        SUM(pmu_counters.pmu_instructions) AS instructions,
        (SUM(pmu_counters.pmu_instructions) * 1.0 / SUM(pmu_counters.pmu_cycles)) AS ipc,
        (SUM(pmu_counters.pmu_branch_misses * 1.0 / pmu_counters.confidence) * 1.0 / SUM(pmu_counters.pmu_branch_instructions * 1.0 / pmu_counters.confidence)) AS branch_miss_rate,
        (SUM(pmu_counters.pmu_branch_misses * 1.0 / pmu_counters.confidence) * 1.0 / SUM(pmu_counters.pmu_instructions) * 1000) AS branch_mpki,
        (SUM(pmu_counters.pmu_llc_misses * 1.0 / pmu_counters.confidence) * 1.0 / (SUM(pmu_counters.pmu_llc_misses * 1.0 / pmu_counters.confidence) + SUM(pmu_counters.pmu_llc_references * 1.0 / pmu_counters.confidence))) AS cache_miss_rate,
        (SUM(pmu_counters.pmu_llc_misses * 1.0 / pmu_counters.confidence) * 1.0 / SUM(pmu_counters.pmu_instructions) * 1000) AS cache_mpki
    FROM pmu_counters
    INNER JOIN proc_map ON pmu_counters.ip = proc_map.ip
    GROUP BY proc_map.func_name;
    ").expect("failed to create a view");
    Ok(())
}

async fn create_roofline_view(connection: &sqlite::Connection) -> Result<()> {
    connection.execute("
CREATE VIEW roofline AS
WITH
ops AS (
  SELECT
    process_id,
    file_name,
    function_name,
    line,
    SUM(bytes_load) AS bytes_load,
    SUM(bytes_store) AS bytes_store,
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
  SELECT
    process_id,
    file_name,
    function_name,
    line,
    SUM(loop_end_ts - loop_start_ts) AS total_duration
  FROM roofline_loop_runs
  GROUP BY process_id, file_name, function_name, line
)
SELECT
  s_file.string AS file_name,
  s_func.string AS function_name,
  runs.line,

  CAST(ops.scalar_int_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_int_ops,
  CAST(ops.scalar_int_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_int_ai,

  CAST(ops.scalar_float_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_float_ops,
  CAST(ops.scalar_float_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_float_ai,

  CAST(ops.scalar_double_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS scalar_double_ops,
  CAST(ops.scalar_double_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_double_ai,

  CAST(ops.vector_int_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_int_ops,
  CAST(ops.vector_int_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_int_ai,

  CAST(ops.vector_float_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_float_ops,
  CAST(ops.vector_float_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_float_ai,

  CAST(ops.vector_double_ops AS REAL) * 1000000000.0 / NULLIF(runs.total_duration, 0) AS vector_double_ops,
  CAST(ops.vector_double_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_double_ai,
  NULL AS timing_samples,
  NULL AS timing_relative_error,
  'legacy' AS timing_quality,
  NULL AS module_offset,
  NULL AS trip_count,
  ops.bytes_load + ops.bytes_store AS arch_bytes,
  NULL AS dram_bytes

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

  CAST(scalar_int_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(scalar_int_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),

  CAST(scalar_float_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(scalar_float_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),

  CAST(scalar_double_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(scalar_double_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),

  CAST(vector_int_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(vector_int_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),

  CAST(vector_float_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(vector_float_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),

  CAST(vector_double_ops AS REAL) * 1000000000.0 / NULLIF(duration_ns, 0),
  CAST(vector_double_ops AS REAL) / NULLIF(arch_bytes_load + arch_bytes_store, 0),
  timing_samples,
  timing_relative_error,
  timing_quality,
  module_offset,
  trip_count,
  arch_bytes_load + arch_bytes_store,
  bytes_load + bytes_store
FROM roofline_binary_loops;
    ").expect("failed to create a view");
    Ok(())
}

#[cfg(test)]
mod metric_tests {
    use super::*;
    use pmu::MetricExpression;
    use sqlite::State;

    #[test]
    fn persists_applicable_derived_metric() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE pmu_counters (
                    pmu_cycles INTEGER,
                    pmu_instructions INTEGER
                );
                INSERT INTO pmu_counters VALUES (100, 175);
                INSERT INTO pmu_counters VALUES (300, 625);",
            )
            .unwrap();
        let metric = pmu::Metric {
            name: "IPC".to_owned(),
            desc: "Instructions per cycle".to_owned(),
            expression: MetricExpression("instructions / cycles".to_owned()),
            unit: Some("insn/cycle".to_owned()),
        };
        persist_metric_definitions(&connection, &[metric]).unwrap();

        let mut statement = connection
            .prepare("SELECT name, value, unit, expression FROM derived_metrics")
            .unwrap();
        assert_eq!(statement.next().unwrap(), State::Row);
        assert_eq!(statement.read::<String, _>("name").unwrap(), "IPC");
        assert_eq!(statement.read::<f64, _>("value").unwrap(), 2.0);
        assert_eq!(statement.read::<String, _>("unit").unwrap(), "insn/cycle");
        assert_eq!(
            statement.read::<String, _>("expression").unwrap(),
            "instructions / cycles"
        );
    }
}

async fn create_tma_view(connection: &sqlite::Connection, info: &ScenarioInfo) -> Result<()> {
    let ScenarioInfo::TMA(info) = info else {
        unreachable!("TMA view requires TMA recording metadata");
    };

    let columns = info
        .metrics
        .iter()
        .map(|metric| {
            let expression =
                pmu_data::arith_parser::try_parse_expr(&metric.formula).map_err(|error| {
                    anyhow::anyhow!("invalid TMA formula '{}': {error}", metric.name)
                })?;
            let marker = metric
                .group
                .as_ref()
                .and_then(|name| {
                    info.groups
                        .iter()
                        .find(|group| &group.name == name)
                        .and_then(|group| {
                            group.events.iter().find(|event| {
                                event.as_str() != "cycles" && event.as_str() != "instructions"
                            })
                        })
                })
                .map(|event| tma_marker_column(&info.counters, event));
            let sql = build_tma_sql_expr(
                &info.metrics,
                &info.counters,
                &info.constants,
                &expression,
                marker.as_deref(),
            );
            Ok::<String, anyhow::Error>(format!("{} AS {}", sql, metric.name.replace('.', "_")))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",\n");

    connection.execute(format!(
        "CREATE VIEW tma AS
         SELECT
             proc_map.func_name AS func_name,
             COUNT(pmu_counters.pmu_cycles) AS num_samples,
             SUM(pmu_counters.pmu_cycles) * 1.0 /
                 NULLIF((SELECT SUM(pmu_cycles) FROM pmu_counters), 0) AS total,
             SUM(pmu_counters.pmu_cycles) AS cycles,
             SUM(pmu_counters.pmu_instructions) AS instructions,
             SUM(pmu_counters.pmu_instructions) * 1.0 /
                 NULLIF(SUM(pmu_counters.pmu_cycles), 0) AS ipc,
             {columns}
         FROM pmu_counters
         INNER JOIN proc_map ON pmu_counters.ip = proc_map.ip
         GROUP BY proc_map.func_name;"
    ))?;
    create_tma_intervals_and_summary(connection, info)?;
    Ok(())
}

fn tma_marker_column(events: &[(EventType, String)], event: &str) -> String {
    events
        .iter()
        .find(|(_, name)| name == event)
        .map(get_event_column_name)
        .unwrap_or_else(|| format!("pmu_{}", event.replace('.', "_")))
}

fn build_tma_sql_expr(
    metrics: &[pmu_data::TmaMetric],
    events: &[(EventType, String)],
    constants: &[pmu_data::TmaConstant],
    expression: &pmu_data::arith_parser::Expr,
    marker: Option<&str>,
) -> String {
    use pmu_data::arith_parser::{BinOp, Expr};

    match expression {
        Expr::Variable(variable) => events
            .iter()
            .find_map(|(event_type, name)| {
                (name == variable).then(|| {
                    let column = get_event_column_name(&(*event_type, name.clone()));
                    let value = if matches!(
                        event_type,
                        EventType::PmuCycles | EventType::PmuInstructions
                    ) {
                        format!("SUM(pmu_counters.{column})")
                    } else {
                        format!("SUM(pmu_counters.{column} / pmu_counters.confidence)")
                    };
                    marker.map_or(value.clone(), |marker| {
                        format!(
                            "SUM(CASE WHEN pmu_counters.{marker} IS NOT NULL THEN ({}) END)",
                            value.trim_start_matches("SUM(").trim_end_matches(')')
                        )
                    })
                })
            })
            .unwrap_or_else(|| {
                let metric = metrics
                    .iter()
                    .find(|metric| metric.name == *variable)
                    .unwrap_or_else(|| panic!("unknown TMA variable '{variable}'"));
                let nested = pmu_data::arith_parser::parse_expr(&metric.formula);
                format!(
                    "({})",
                    build_tma_sql_expr(metrics, events, constants, &nested, marker)
                )
            }),
        Expr::Constant(name) => constants
            .iter()
            .find(|constant| constant.name == *name)
            // A missing constant must make the metric unavailable, never turn
            // into a plausible-looking zero-valued result.
            .map_or_else(|| "NULL".to_string(), |constant| constant.value.to_string()),
        Expr::Binary { op, lhs, rhs } => {
            let lhs = build_tma_sql_expr(metrics, events, constants, lhs, marker);
            let rhs = build_tma_sql_expr(metrics, events, constants, rhs, marker);
            match op {
                BinOp::Add => format!("({lhs}) + ({rhs})"),
                BinOp::Sub => format!("({lhs}) - ({rhs})"),
                BinOp::Mul => format!("({lhs}) * ({rhs})"),
                BinOp::Div => format!("CAST(({lhs}) AS REAL) / NULLIF(CAST(({rhs}) AS REAL), 0)"),
                BinOp::Eq => format!("({lhs}) = ({rhs})"),
                BinOp::Lt => format!("({lhs}) < ({rhs})"),
                BinOp::Le => format!("({lhs}) <= ({rhs})"),
                BinOp::Gt => format!("({lhs}) > ({rhs})"),
                BinOp::Ge => format!("({lhs}) >= ({rhs})"),
            }
        }
        Expr::Call { name, args } => {
            let args = args
                .iter()
                .map(|arg| build_tma_sql_expr(metrics, events, constants, arg, marker))
                .collect::<Vec<_>>();
            match name.to_ascii_lowercase().as_str() {
                "min" if args.len() == 2 => format!("MIN({}, {})", args[0], args[1]),
                "max" if args.len() == 2 => format!("MAX({}, {})", args[0], args[1]),
                "abs" if args.len() == 1 => format!("ABS({})", args[0]),
                "if" if args.len() == 3 => format!(
                    "CASE WHEN ({}) <> 0 THEN ({}) ELSE ({}) END",
                    args[0], args[1], args[2]
                ),
                _ => "NULL".to_owned(),
            }
        }
        Expr::Num(number) => number.to_string(),
    }
}
