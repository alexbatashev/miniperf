use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use sqlite::{Connection, Value};

#[derive(Debug, Clone, Default)]
pub(crate) struct ProfileData {
    pub(crate) frames: Vec<ProfileFrame>,
    pub(crate) samples: Vec<ProfileSample>,
    pub(crate) cpu_observations: Vec<CpuObservation>,
    /// Source-host topology, present only in recordings that preserved it.
    pub(crate) logical_cpu_count: Option<u32>,
    pub(crate) counter_metrics: Vec<CounterMetric>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileFrame {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) module: Option<String>,
    pub(crate) file: Option<String>,
    pub(crate) line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProfileSample {
    pub(crate) timestamp_ns: u64,
    pub(crate) process_id: u32,
    pub(crate) thread_id: u32,
    pub(crate) cpu: Option<u32>,
    /// Interned frame IDs ordered from the root to the sampled leaf.
    pub(crate) stack: Vec<usize>,
    /// Values aligned with [`ProfileData::counter_metrics`].
    pub(crate) counters: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CpuObservationSource {
    CounterDelta,
    SampledOccupancy,
    LegacyUnknown,
}

impl CpuObservationSource {
    fn parse(value: &str) -> Self {
        match value {
            "counter_delta" => Self::CounterDelta,
            "sampled_occupancy" => Self::SampledOccupancy,
            _ => Self::LegacyUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CpuObservation {
    pub(crate) timestamp_ns: u64,
    pub(crate) process_id: u32,
    pub(crate) thread_id: u32,
    pub(crate) cpu: Option<u32>,
    /// Start of the half-open wall-time interval ending at `timestamp_ns`.
    /// Present for current counter-delta recordings; sampled occupancy is a
    /// point observation and legacy recordings did not preserve this value.
    pub(crate) interval_start_ns: Option<u64>,
    /// Interned frame IDs ordered from the root to the sampled leaf. This may
    /// be empty when CPU occupancy was observed but stack collection failed.
    pub(crate) stack: Vec<usize>,
    pub(crate) weight_ns: u64,
    pub(crate) source: CpuObservationSource,
}

impl CpuObservation {
    pub(crate) fn wall_interval_start_ns(&self) -> Option<u64> {
        match self.source {
            CpuObservationSource::CounterDelta => self.interval_start_ns,
            CpuObservationSource::LegacyUnknown => {
                Some(self.timestamp_ns.saturating_sub(self.weight_ns))
            }
            CpuObservationSource::SampledOccupancy => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CounterMetric {
    pub(crate) key: String,
    pub(crate) label: String,
}

/// A half-open timestamp interval: `[start_ns, end_ns)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeRange {
    pub(crate) start_ns: u64,
    pub(crate) end_ns: u64,
}

impl ProfileData {
    pub(crate) fn load(connection: &Connection) -> Self {
        match load_profile(connection) {
            Ok(profile) => profile,
            Err(error) => Self {
                error: Some(format!("{error:#}")),
                ..Self::default()
            },
        }
    }

    pub(crate) fn full_range(&self) -> Option<TimeRange> {
        let start_ns = self
            .samples
            .iter()
            .map(|sample| sample.timestamp_ns)
            .chain(
                self.cpu_observations
                    .iter()
                    .map(|observation| observation.timestamp_ns),
            )
            .chain(
                self.cpu_observations
                    .iter()
                    .filter_map(CpuObservation::wall_interval_start_ns),
            )
            .min()?;
        let latest_ns = self
            .samples
            .iter()
            .map(|sample| sample.timestamp_ns)
            .chain(
                self.cpu_observations
                    .iter()
                    .map(|observation| observation.timestamp_ns),
            )
            .max()?;

        Some(TimeRange {
            start_ns,
            end_ns: latest_ns.saturating_add(1),
        })
    }
}

#[derive(Debug)]
struct SqliteColumn {
    name: String,
    declared_type: String,
}

#[derive(Debug, Clone)]
struct ResolvedFrame {
    name: String,
    module: Option<String>,
    file: Option<String>,
    line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FrameKey {
    name: String,
    module: Option<String>,
}

fn load_profile(connection: &Connection) -> Result<ProfileData> {
    let sample_columns = table_columns(connection, "pmu_counters")?;
    if sample_columns.is_empty() {
        bail!("timestamped profiles are unavailable: SQLite table `pmu_counters` does not exist");
    }

    let timestamp_column = required_column(&sample_columns, "timestamp")?;
    let process_column = required_column(&sample_columns, "process_id")?;
    let thread_column = required_column(&sample_columns, "thread_id")?;
    let call_stack_column = required_column(&sample_columns, "call_stack")?;
    let cpu_column = find_column(&sample_columns, "cpu");

    let dynamic_columns = sample_columns
        .iter()
        .filter(|column| !is_sample_metadata(&column.name))
        .filter(|column| has_numeric_affinity(&column.declared_type))
        .collect::<Vec<_>>();
    let counter_metrics = dynamic_columns
        .iter()
        .map(|column| CounterMetric {
            key: column.name.clone(),
            label: counter_label(&column.name),
        })
        .collect::<Vec<_>>();

    let resolved_frames = load_proc_map(connection)?;
    let mut frames = Vec::new();
    let mut frame_ids = HashMap::<FrameKey, usize>::new();

    let mut selected_columns = vec![
        quoted(timestamp_column),
        quoted(process_column),
        quoted(thread_column),
        quoted(call_stack_column),
    ];
    selected_columns.push(
        cpu_column
            .map(quoted)
            .unwrap_or_else(|| "NULL AS cpu".to_string()),
    );
    selected_columns.extend(dynamic_columns.iter().map(|column| quoted(&column.name)));

    let query = format!(
        "SELECT {} FROM pmu_counters ORDER BY {} ASC;",
        selected_columns.join(", "),
        quoted(timestamp_column)
    );
    let statement = connection
        .prepare(query)
        .context("failed to query timestamped profile samples")?;
    let mut samples = Vec::new();

    for row in statement.into_iter() {
        let row = row.context("failed to read a timestamped profile sample")?;
        let timestamp_ns = required_nonnegative_u64(&row[0], "pmu_counters.timestamp")?;
        let process_id = required_u32(&row[1], "pmu_counters.process_id")?;
        let thread_id = required_u32(&row[2], "pmu_counters.thread_id")?;
        let raw_stack = parse_call_stack(&row[3], "pmu_counters.call_stack")?;
        let cpu = optional_cpu(&row[4]);
        let stack = intern_stack(raw_stack, &resolved_frames, &mut frames, &mut frame_ids);

        let counters = dynamic_columns
            .iter()
            .enumerate()
            .map(|(offset, column)| numeric_counter(&row[5 + offset], &column.name))
            .collect::<Result<Vec<_>>>()?;

        samples.push(ProfileSample {
            timestamp_ns,
            process_id,
            thread_id,
            cpu,
            stack,
            counters,
        });
    }

    let cpu_observations = load_cpu_observations(
        connection,
        &samples,
        &counter_metrics,
        &resolved_frames,
        &mut frames,
        &mut frame_ids,
    )?;

    Ok(ProfileData {
        frames,
        samples,
        cpu_observations,
        logical_cpu_count: None,
        counter_metrics,
        error: None,
    })
}

fn load_cpu_observations(
    connection: &Connection,
    samples: &[ProfileSample],
    counter_metrics: &[CounterMetric],
    resolved_frames: &HashMap<u64, ResolvedFrame>,
    frames: &mut Vec<ProfileFrame>,
    frame_ids: &mut HashMap<FrameKey, usize>,
) -> Result<Vec<CpuObservation>> {
    let columns = table_columns(connection, "cpu_observations")?;
    if columns.is_empty() {
        return Ok(legacy_cpu_observations(samples, counter_metrics));
    }

    let process_column = required_column(&columns, "process_id")?;
    let thread_column = required_column(&columns, "thread_id")?;
    let cpu_column = required_column(&columns, "cpu")?;
    let timestamp_column = required_column(&columns, "timestamp")?;
    let interval_start_column = find_column(&columns, "interval_start_ns");
    let weight_column = required_column(&columns, "weight_ns")?;
    let source_column = required_column(&columns, "source")?;
    let call_stack_column = required_column(&columns, "call_stack")?;
    let selected_columns = [
        quoted(timestamp_column),
        quoted(process_column),
        quoted(thread_column),
        quoted(cpu_column),
        interval_start_column
            .map(quoted)
            .unwrap_or_else(|| "NULL AS interval_start_ns".to_string()),
        quoted(weight_column),
        quoted(source_column),
        quoted(call_stack_column),
    ];
    let query = format!(
        "SELECT {} FROM cpu_observations ORDER BY {} ASC;",
        selected_columns.join(", "),
        quoted(timestamp_column)
    );
    let statement = connection
        .prepare(query)
        .context("failed to query CPU observations")?;
    let mut observations = Vec::new();
    for row in statement.into_iter() {
        let row = row.context("failed to read a CPU observation")?;
        let timestamp_ns = required_nonnegative_u64(&row[0], "cpu_observations.timestamp")?;
        let process_id = required_u32(&row[1], "cpu_observations.process_id")?;
        let thread_id = required_u32(&row[2], "cpu_observations.thread_id")?;
        let cpu = optional_cpu(&row[3]);
        let interval_start_ns =
            optional_nonnegative_u64(&row[4], "cpu_observations.interval_start_ns")?;
        let weight_ns = required_nonnegative_u64(&row[5], "cpu_observations.weight_ns")?;
        let mut source = optional_text(&row[6], "cpu_observations.source")?
            .map(|source| CpuObservationSource::parse(&source))
            .unwrap_or(CpuObservationSource::LegacyUnknown);
        if interval_start_column.is_none() && source == CpuObservationSource::CounterDelta {
            source = CpuObservationSource::LegacyUnknown;
        }
        let raw_stack = parse_call_stack(&row[7], "cpu_observations.call_stack")?;
        let stack = intern_stack(raw_stack, resolved_frames, frames, frame_ids);
        observations.push(CpuObservation {
            timestamp_ns,
            process_id,
            thread_id,
            cpu,
            interval_start_ns,
            stack,
            weight_ns,
            source,
        });
    }
    Ok(observations)
}

fn legacy_cpu_observations(
    samples: &[ProfileSample],
    counter_metrics: &[CounterMetric],
) -> Vec<CpuObservation> {
    let Some(cpu_clock_index) = counter_metrics
        .iter()
        .position(|metric| metric.key.eq_ignore_ascii_case("os_cpu_clock"))
    else {
        return Vec::new();
    };
    samples
        .iter()
        .filter_map(|sample| {
            let weight = sample
                .counters
                .get(cpu_clock_index)
                .and_then(|value| *value)
                .filter(|value| value.is_finite() && *value > 0.0)?
                .min(u64::MAX as f64)
                .round() as u64;
            (weight > 0).then(|| CpuObservation {
                timestamp_ns: sample.timestamp_ns,
                process_id: sample.process_id,
                thread_id: sample.thread_id,
                cpu: sample.cpu,
                interval_start_ns: None,
                stack: sample.stack.clone(),
                weight_ns: weight,
                source: CpuObservationSource::LegacyUnknown,
            })
        })
        .collect()
}

fn intern_stack(
    raw_stack: Vec<u64>,
    resolved_frames: &HashMap<u64, ResolvedFrame>,
    frames: &mut Vec<ProfileFrame>,
    frame_ids: &mut HashMap<FrameKey, usize>,
) -> Vec<usize> {
    raw_stack
        .into_iter()
        .rev()
        .map(|ip| {
            let resolved = resolved_frames
                .get(&ip)
                .cloned()
                .unwrap_or_else(|| unresolved_frame(ip));
            intern_frame(resolved, frames, frame_ids)
        })
        .collect()
}

fn load_proc_map(connection: &Connection) -> Result<HashMap<u64, ResolvedFrame>> {
    let columns = table_columns(connection, "proc_map")?;
    if columns.is_empty() {
        bail!("timestamped profiles are unavailable: SQLite table `proc_map` does not exist");
    }

    let ip_column = required_column(&columns, "ip")?;
    let function_column = required_column(&columns, "func_name")?;
    let module_column = find_column(&columns, "module_path");
    let file_column = find_column(&columns, "file_name");
    let line_column = find_column(&columns, "line");

    let selected_columns = [
        quoted(ip_column),
        quoted(function_column),
        module_column
            .map(quoted)
            .unwrap_or_else(|| "NULL AS module_path".to_string()),
        file_column
            .map(quoted)
            .unwrap_or_else(|| "NULL AS file_name".to_string()),
        line_column
            .map(quoted)
            .unwrap_or_else(|| "NULL AS line".to_string()),
    ];
    let query = format!("SELECT {} FROM proc_map;", selected_columns.join(", "));
    let statement = connection
        .prepare(query)
        .context("failed to query profile symbols from `proc_map`")?;
    let mut resolved = HashMap::new();

    for row in statement.into_iter() {
        let row = row.context("failed to read a profile symbol from `proc_map`")?;
        let ip = sqlite_ip(&row[0]).context("invalid `proc_map.ip`")?;
        let name = optional_text(&row[1], "proc_map.func_name")?
            .filter(|name| name != "[unknown]")
            .unwrap_or_else(|| unknown_name(ip));
        let candidate = ResolvedFrame {
            name,
            module: optional_text(&row[2], "proc_map.module_path")?,
            file: optional_text(&row[3], "proc_map.file_name")?.filter(|file| file != "unknown"),
            line: optional_positive_u32(&row[4]),
        };

        resolved
            .entry(ip)
            .and_modify(|current: &mut ResolvedFrame| {
                fill_missing_source(current, &candidate);
            })
            .or_insert(candidate);
    }

    Ok(resolved)
}

fn intern_frame(
    candidate: ResolvedFrame,
    frames: &mut Vec<ProfileFrame>,
    frame_ids: &mut HashMap<FrameKey, usize>,
) -> usize {
    let key = FrameKey {
        name: candidate.name.clone(),
        module: candidate.module.clone(),
    };
    if let Some(&id) = frame_ids.get(&key) {
        let current = &mut frames[id];
        if current.file.is_none() {
            current.file = candidate.file;
        }
        if current.line.is_none() {
            current.line = candidate.line;
        }
        return id;
    }

    let id = frames.len();
    frames.push(ProfileFrame {
        id,
        name: candidate.name,
        module: candidate.module,
        file: candidate.file,
        line: candidate.line,
    });
    frame_ids.insert(key, id);
    id
}

fn fill_missing_source(current: &mut ResolvedFrame, candidate: &ResolvedFrame) {
    if current.file.is_none() {
        current.file.clone_from(&candidate.file);
    }
    if current.line.is_none() {
        current.line = candidate.line;
    }
}

fn unresolved_frame(ip: u64) -> ResolvedFrame {
    ResolvedFrame {
        name: unknown_name(ip),
        module: None,
        file: None,
        line: None,
    }
}

fn unknown_name(ip: u64) -> String {
    format!("0x{ip:x}")
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<SqliteColumn>> {
    let query = format!("PRAGMA table_info({});", quoted(table));
    connection
        .prepare(query)
        .with_context(|| format!("failed to inspect SQLite table `{table}`"))?
        .into_iter()
        .map(|row| {
            let row = row.with_context(|| format!("failed to inspect SQLite table `{table}`"))?;
            Ok(SqliteColumn {
                name: row
                    .try_read::<&str, _>("name")
                    .context("missing column name in SQLite schema")?
                    .to_string(),
                declared_type: row
                    .try_read::<&str, _>("type")
                    .context("missing column type in SQLite schema")?
                    .to_string(),
            })
        })
        .collect()
}

fn required_column<'a>(columns: &'a [SqliteColumn], expected: &str) -> Result<&'a str> {
    find_column(columns, expected).with_context(|| {
        format!(
            "timestamped profiles are unavailable in this legacy recording: \
             `pmu_counters`/`proc_map` is missing required column `{expected}`"
        )
    })
}

fn find_column<'a>(columns: &'a [SqliteColumn], expected: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(expected))
        .map(|column| column.name.as_str())
}

fn is_sample_metadata(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "unique_id"
            | "process_id"
            | "thread_id"
            | "cpu"
            | "time_enabled"
            | "time_running"
            | "confidence"
            | "timestamp"
            | "ip"
            | "call_stack"
    )
}

fn has_numeric_affinity(declared_type: &str) -> bool {
    let declared_type = declared_type.to_ascii_uppercase();
    declared_type.contains("INT")
        || declared_type.contains("REAL")
        || declared_type.contains("FLOA")
        || declared_type.contains("DOUB")
        || declared_type.contains("NUM")
        || declared_type.contains("DEC")
}

fn counter_label(key: &str) -> String {
    let words = key
        .strip_prefix("pmu_")
        .unwrap_or(key)
        .split('_')
        .filter(|word| !word.is_empty())
        .enumerate()
        .map(|(index, word)| match word.to_ascii_lowercase().as_str() {
            "cpu" | "fp" | "ipc" | "llc" | "mpki" | "simd" | "tlb" | "dtlb" | "itlb" | "l1"
            | "l1d" | "l1i" | "l2" | "l3" => word.to_ascii_uppercase(),
            _ if index == 0 => {
                let mut characters = word.chars();
                match characters.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                    None => String::new(),
                }
            }
            _ => word.to_string(),
        })
        .collect::<Vec<_>>();

    if words.is_empty() {
        key.to_string()
    } else {
        words.join(" ")
    }
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn parse_call_stack(value: &Value, field: &str) -> Result<Vec<u64>> {
    match value {
        Value::String(stack) => serde_json::from_str(stack)
            .with_context(|| format!("invalid `{field}` value `{stack}`")),
        Value::Null => Ok(Vec::new()),
        _ => bail!("`{field}` is not text"),
    }
}

fn sqlite_ip(value: &Value) -> Result<u64> {
    match value {
        // SQLite only has signed integers. Casting reconstructs raw addresses
        // that were persisted from `u64` with `as i64`.
        Value::Integer(value) => Ok(*value as u64),
        Value::Float(value) if value.is_finite() && value.fract() == 0.0 => Ok(*value as u64),
        Value::String(value) => value
            .parse()
            .with_context(|| format!("invalid instruction pointer `{value}`")),
        _ => bail!("instruction pointer is not an integer"),
    }
}

fn required_nonnegative_u64(value: &Value, field: &str) -> Result<u64> {
    match value {
        Value::Integer(value) if *value >= 0 => Ok(*value as u64),
        Value::Float(value) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => {
            Ok(*value as u64)
        }
        _ => bail!("`{field}` is not a non-negative integer"),
    }
}

fn optional_nonnegative_u64(value: &Value, field: &str) -> Result<Option<u64>> {
    match value {
        Value::Null => Ok(None),
        _ => required_nonnegative_u64(value, field).map(Some),
    }
}

fn required_u32(value: &Value, field: &str) -> Result<u32> {
    let value = required_nonnegative_u64(value, field)?;
    u32::try_from(value).with_context(|| format!("`{field}` is outside the u32 range"))
}

fn optional_cpu(value: &Value) -> Option<u32> {
    match value {
        Value::Integer(value) if (0..u32::MAX as i64).contains(value) => Some(*value as u32),
        Value::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 0.0
                && *value < u32::MAX as f64 =>
        {
            Some(*value as u32)
        }
        _ => None,
    }
}

fn numeric_counter(value: &Value, column: &str) -> Result<Option<f64>> {
    match value {
        Value::Null => Ok(None),
        Value::Integer(value) => Ok(Some(*value as f64)),
        Value::Float(value) => Ok(Some(*value)),
        _ => bail!("numeric counter `{column}` contains a non-numeric value"),
    }
}

fn optional_text(value: &Value, field: &str) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) if value.trim().is_empty() => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        _ => bail!("`{field}` is not text"),
    }
}

fn optional_positive_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Integer(value) => u32::try_from(*value).ok().filter(|value| *value > 0),
        Value::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value > 0.0
                && *value <= u32::MAX as f64 =>
        {
            Some(*value as u32)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_root_to_leaf_interned_stacks_and_dynamic_nullable_counters() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "
                CREATE TABLE proc_map (
                    ip INTEGER,
                    func_name TEXT,
                    file_name TEXT,
                    line INTEGER,
                    module_path TEXT
                );
                INSERT INTO proc_map VALUES (16, 'leaf', '', NULL, '/app');
                INSERT INTO proc_map VALUES (17, 'leaf', '/src/leaf.rs', 12, '/app');
                INSERT INTO proc_map VALUES (32, 'root', '/src/root.rs', 4, '/app');

                CREATE TABLE pmu_counters (
                    timestamp INTEGER NOT NULL,
                    process_id INTEGER NOT NULL,
                    thread_id INTEGER NOT NULL,
                    call_stack TEXT,
                    cpu INTEGER,
                    pmu_cycles INTEGER,
                    custom_rate REAL,
                    annotation TEXT
                );
                INSERT INTO pmu_counters
                    VALUES (100, 7, 9, '[16, 32]', 3, 10, NULL, 'first');
                INSERT INTO pmu_counters
                    VALUES (200, 7, 9, '[17, 32]', 4, NULL, 2.5, 'second');
                ",
            )
            .unwrap();

        let profile = ProfileData::load(&connection);

        assert_eq!(profile.error, None);
        assert_eq!(
            profile
                .counter_metrics
                .iter()
                .map(|metric| metric.key.as_str())
                .collect::<Vec<_>>(),
            ["pmu_cycles", "custom_rate"]
        );
        assert_eq!(
            profile
                .counter_metrics
                .iter()
                .map(|metric| metric.label.as_str())
                .collect::<Vec<_>>(),
            ["Cycles", "Custom rate"]
        );
        assert_eq!(profile.frames.len(), 2);

        let first_names = profile.samples[0]
            .stack
            .iter()
            .map(|id| profile.frames[*id].name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(first_names, ["root", "leaf"]);
        assert_eq!(profile.samples[0].stack, profile.samples[1].stack);

        let leaf = profile
            .frames
            .iter()
            .find(|frame| frame.name == "leaf")
            .unwrap();
        assert_eq!(leaf.file.as_deref(), Some("/src/leaf.rs"));
        assert_eq!(leaf.line, Some(12));
        assert_eq!(profile.samples[0].counters, [Some(10.0), None]);
        assert_eq!(profile.samples[1].counters, [None, Some(2.5)]);
        assert_eq!(
            profile.full_range(),
            Some(TimeRange {
                start_ns: 100,
                end_ns: 201
            })
        );
        let first_only = TimeRange {
            start_ns: 100,
            end_ns: 200,
        };
        assert_eq!(
            profile
                .samples
                .iter()
                .filter(|sample| {
                    first_only.start_ns <= sample.timestamp_ns
                        && sample.timestamp_ns < first_only.end_ns
                })
                .count(),
            1
        );
        assert_eq!(
            profile
                .samples
                .iter()
                .find(|sample| {
                    first_only.start_ns <= sample.timestamp_ns
                        && sample.timestamp_ns < first_only.end_ns
                })
                .map(|sample| sample.timestamp_ns),
            Some(100)
        );
    }

    #[test]
    fn cpu_column_is_capability_detected_and_sentinels_are_unknown() {
        let legacy = sqlite::open(":memory:").unwrap();
        legacy
            .execute(
                "
                CREATE TABLE proc_map (ip INTEGER, func_name TEXT);
                CREATE TABLE pmu_counters (
                    timestamp INTEGER,
                    process_id INTEGER,
                    thread_id INTEGER,
                    call_stack TEXT,
                    pmu_cycles INTEGER
                );
                INSERT INTO pmu_counters VALUES (1, 2, 3, '[]', 4);
                ",
            )
            .unwrap();
        let legacy_profile = ProfileData::load(&legacy);
        assert_eq!(legacy_profile.error, None);
        assert_eq!(legacy_profile.samples[0].cpu, None);

        let current = sqlite::open(":memory:").unwrap();
        current
            .execute(
                "
                CREATE TABLE proc_map (ip INTEGER, func_name TEXT);
                CREATE TABLE pmu_counters (
                    timestamp INTEGER,
                    process_id INTEGER,
                    thread_id INTEGER,
                    call_stack TEXT,
                    cpu INTEGER,
                    pmu_cycles INTEGER
                );
                INSERT INTO pmu_counters VALUES (1, 2, 3, '[]', 4, 10);
                INSERT INTO pmu_counters VALUES (2, 2, 3, '[]', -1, 10);
                INSERT INTO pmu_counters VALUES (3, 2, 3, '[]', 4294967295, 10);
                ",
            )
            .unwrap();
        let current_profile = ProfileData::load(&current);
        assert_eq!(current_profile.error, None);
        assert_eq!(
            current_profile
                .samples
                .iter()
                .map(|sample| sample.cpu)
                .collect::<Vec<_>>(),
            [Some(4), None, None]
        );
    }

    #[test]
    fn loads_stack_independent_cpu_observations_with_source_semantics() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "
                CREATE TABLE proc_map (ip INTEGER, func_name TEXT);
                INSERT INTO proc_map VALUES (16, 'leaf');
                CREATE TABLE pmu_counters (
                    timestamp INTEGER,
                    process_id INTEGER,
                    thread_id INTEGER,
                    call_stack TEXT,
                    cpu INTEGER,
                    os_cpu_clock INTEGER
                );
                INSERT INTO pmu_counters VALUES (100, 2, 3, '[16]', 4, 10);
                CREATE TABLE cpu_observations (
                    process_id INTEGER,
                    thread_id INTEGER,
                    cpu INTEGER,
                    timestamp INTEGER,
                    interval_start_ns INTEGER,
                    weight_ns INTEGER,
                    source TEXT,
                    call_stack TEXT
                );
                INSERT INTO cpu_observations
                    VALUES (2, 3, 7, 110, NULL, 20, 'sampled_occupancy', '[]');
                INSERT INTO cpu_observations
                    VALUES (2, 3, 8, 120, NULL, 20, 'sampled_occupancy', '[16]');
                INSERT INTO cpu_observations
                    VALUES (2, 3, 8, 130, 100, 5, 'counter_delta', '[16]');
                ",
            )
            .unwrap();

        let profile = ProfileData::load(&connection);

        assert_eq!(profile.error, None);
        assert_eq!(profile.cpu_observations.len(), 3);
        assert_eq!(profile.cpu_observations[0].cpu, Some(7));
        assert!(profile.cpu_observations[0].stack.is_empty());
        assert_eq!(
            profile.cpu_observations[0].source,
            CpuObservationSource::SampledOccupancy
        );
        assert_eq!(profile.cpu_observations[1].stack, vec![0]);
        assert_eq!(profile.cpu_observations[1].weight_ns, 20);
        assert_eq!(
            profile.cpu_observations[2].source,
            CpuObservationSource::CounterDelta
        );
        assert_eq!(profile.cpu_observations[2].interval_start_ns, Some(100));
    }

    #[test]
    fn legacy_counter_delta_without_wall_interval_keeps_compatibility_semantics() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "
                CREATE TABLE proc_map (ip INTEGER, func_name TEXT);
                CREATE TABLE pmu_counters (
                    timestamp INTEGER,
                    process_id INTEGER,
                    thread_id INTEGER,
                    call_stack TEXT,
                    cpu INTEGER,
                    os_cpu_clock INTEGER
                );
                CREATE TABLE cpu_observations (
                    process_id INTEGER,
                    thread_id INTEGER,
                    cpu INTEGER,
                    timestamp INTEGER,
                    weight_ns INTEGER,
                    source TEXT,
                    call_stack TEXT
                );
                INSERT INTO cpu_observations
                    VALUES (2, 3, 7, 110, 20, 'counter_delta', '[]');
                ",
            )
            .unwrap();

        let profile = ProfileData::load(&connection);

        assert_eq!(profile.error, None);
        assert_eq!(profile.cpu_observations.len(), 1);
        assert_eq!(
            profile.cpu_observations[0].source,
            CpuObservationSource::LegacyUnknown
        );
        assert_eq!(profile.cpu_observations[0].interval_start_ns, None);
    }

    #[test]
    fn missing_or_legacy_schema_is_reported_without_panicking() {
        let missing = sqlite::open(":memory:").unwrap();
        let missing_profile = ProfileData::load(&missing);
        assert!(missing_profile.samples.is_empty());
        assert!(missing_profile
            .error
            .as_deref()
            .unwrap()
            .contains("pmu_counters"));

        let legacy = sqlite::open(":memory:").unwrap();
        legacy
            .execute(
                "
                CREATE TABLE proc_map (ip INTEGER, func_name TEXT);
                CREATE TABLE pmu_counters (
                    timestamp INTEGER,
                    process_id INTEGER,
                    thread_id INTEGER,
                    pmu_cycles INTEGER
                );
                ",
            )
            .unwrap();
        let legacy_profile = ProfileData::load(&legacy);
        assert!(legacy_profile.samples.is_empty());
        assert!(legacy_profile
            .error
            .as_deref()
            .unwrap()
            .contains("call_stack"));
    }
}
