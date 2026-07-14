use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::Result;
use kdam::BarExt;
use memmap2::{Advice, Mmap};
use mperf_data::{
    CallFrame, Event, EventType, IString, ProcMapEntry, RecordInfo, Scenario, ScenarioInfo,
};
use object::{Object, ObjectSymbol, SymbolKind};
use smallvec::SmallVec;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

use crate::disassembly::{default_disassembler, DisassembleRequest, DisassembleTarget};
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
            process_pmu_counters(&connection, &info.scenario_info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
        }
        Scenario::Roofline => {
            process_pmu_counters(&connection, &info.scenario_info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
            create_roofline_view(&connection).await?;
        }
        Scenario::TMA => {
            process_pmu_counters(&connection, &info.scenario_info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_tma_view(&connection, &info.scenario_info).await?;
        }
    }

    persist_derived_metrics(&connection)?;

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
            statement.bind((1, s.id as f64))?;
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
    info: &ScenarioInfo,
    res_dir: &Path,
    pb: &mut kdam::Bar,
) -> Result<()> {
    let events = match info {
        ScenarioInfo::Snapshot(s) => &s.counters,
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
                time_enabled INTEGER NOT NULL,
                time_running INTEGER NOT NULL,
                confidence REAL NOT NULL,
                timestamp INTEGER NOT NULL,
                ip INTEGER NOT NULL,
                call_stack TEXT{}
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
    let clusters: Vec<ClusterRanges> = {
        let data = std::fs::read_to_string(res_dir.join("info.json"))?;
        let ri: RecordInfo = serde_json::from_str(&data)?;
        ri.cores
            .into_iter()
            .map(|c| (c.family_id, c.name, parse_cpumask(&c.cpus)))
            .collect()
    };

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
    let placeholders = std::iter::repeat_n("?", 9 + event_columns.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut counter_stmt = connection.prepare(format!(
        "INSERT INTO pmu_counters (
            unique_id, process_id, thread_id, time_enabled, time_running,
            confidence, timestamp, ip, call_stack{insert_columns}
         ) VALUES ({placeholders});"
    ))?;

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
            } else if evt.ty == EventType::PmuInstructions && !folded_stack.is_empty() {
                if let Some(weight) = flamegraph_sample_weight(evt.value) {
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
            }

            let event_name = strings.get(&evt.name).cloned().unwrap_or_default();
            counters.insert(get_event_column_name(&(evt.ty, event_name)), evt.value);
        }

        if let Some(lead_event) = &lead_event {
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
        run_stmt.bind((1, run.id as f64))?;
        run_stmt.bind((2, run.pid as i64))?;
        run_stmt.bind((3, run.tid as i64))?;
        run_stmt.bind((4, run.file_name as f64))?;
        run_stmt.bind((5, run.func_name as f64))?;
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
        ops_stmt.bind((1, ops.id as f64))?;
        ops_stmt.bind((2, ops.pid as i64))?;
        ops_stmt.bind((3, ops.tid as i64))?;
        ops_stmt.bind((4, ops.file_name as f64))?;
        ops_stmt.bind((5, ops.func_name as f64))?;
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
    let call_stack = format!(
        "[{}]",
        lead_event
            .callstack
            .iter()
            .map(|frame| frame.as_ip().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    statement.reset()?;
    statement.bind((1, lead_event.unique_id as f64))?;
    statement.bind((2, lead_event.process_id as i64))?;
    statement.bind((3, lead_event.thread_id as i64))?;
    statement.bind((4, lead_event.time_enabled as i64))?;
    statement.bind((5, lead_event.time_running as i64))?;
    statement.bind((6, confidence))?;
    statement.bind((7, lead_event.timestamp as i64))?;
    statement.bind((
        8,
        lead_event.callstack.first().map(|f| f.as_ip()).unwrap_or(0) as i64,
    ))?;
    statement.bind((9, call_stack.as_str()))?;
    for (offset, column) in event_columns.iter().enumerate() {
        let value = counters
            .get(column)
            .copied()
            .map(|value| value as i64)
            .or_else(|| (!missing_is_null).then_some(0));
        statement.bind((10 + offset, value))?;
    }
    statement.next()?;
    Ok(())
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
    use super::{counter_group_has_profile_data, CounterLead};
    use mperf_data::CallFrame;
    use smallvec::SmallVec;
    use std::collections::HashMap;

    fn event(callstack: SmallVec<[CallFrame; 32]>) -> CounterLead {
        CounterLead {
            unique_id: 1,
            correlation_id: 1,
            thread_id: 1,
            process_id: 1,
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
    use super::{populate_assembly_samples, sampled_disassembly_targets, RooflineData};
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
    fn sampled_symbol_selection_avoids_unrelated_object_code() {
        let executable = std::env::current_exe().unwrap();
        let bytes = std::fs::read(&executable).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = object
            .symbols()
            .find(|symbol| {
                symbol.kind() == SymbolKind::Text
                    && symbol.address() != 0
                    && symbol.size() > 2
                    && symbol.name().is_ok()
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
            perf_pid: 10,
            counters: Vec::new(),
            inst_pid: 20,
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
  CAST(ops.vector_double_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_double_ai

FROM runs
LEFT JOIN ops
  ON runs.file_name = ops.file_name
  AND runs.function_name = ops.function_name
  AND runs.line = ops.line
LEFT JOIN strings s_file ON runs.file_name = s_file.id
LEFT JOIN strings s_func ON runs.function_name = s_func.id;
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
