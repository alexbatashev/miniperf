use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::Result;
use memmap2::{Advice, Mmap};
use mperf_data::{
    CallFrame, Event, EventType, IString, ProcMap, RecordInfo, Scenario, ScenarioInfo,
};
use tokio::fs::{self, File};

use crate::utils;

pub async fn perform_postprocessing(res_dir: &Path) -> Result<()> {
    let data = fs::read_to_string(res_dir.join("info.json"))
        .await
        .expect("failed to read info.json");
    let info: RecordInfo = serde_json::from_str(&data).expect("failed to parse info.json");

    let connection = sqlite::open(res_dir.join("perf.db"))?;
    connection.execute(
        "
            CREATE TABLE proc_map (ip INTEGER, func_name TEXT, file_name TEXT, line INTEGER);
            CREATE TABLE strings (id BINARY(128) NOT NULL, string TEXT NOT NULL);
        ",
    )?;

    process_strings(&connection, res_dir).await?;

    match info.scenario {
        Scenario::Snapshot => {
            process_pmu_counters(&connection, &info.scenario_info, res_dir).await?;
            create_hotspots_view(&connection).await?;
        }
        Scenario::Roofline => {
            process_pmu_counters(&connection, &info.scenario_info, res_dir).await?;
            process_roofline_events(&connection, &info.scenario_info, res_dir).await?;
            create_hotspots_view(&connection).await?;
            create_roofline_view(&connection).await?;
        }
    }

    Ok(())
}

async fn process_strings(connection: &sqlite::Connection, res_dir: &Path) -> Result<()> {
    let strings_file =
        std::fs::File::open(res_dir.join("strings.json")).expect("failed to open strings.json");
    let strings: Vec<IString> =
        serde_json::from_reader(strings_file).expect("failed to parse strings.json");

    for s in strings {
        connection.execute(format!(
            "INSERT INTO strings (id, string) VALUES ({}, '{}');",
            s.id as u128, s.value
        ))?;
    }

    Ok(())
}
async fn process_pmu_counters(
    connection: &sqlite::Connection,
    info: &ScenarioInfo,
    res_dir: &Path,
) -> Result<()> {
    let events = match info {
        ScenarioInfo::Snapshot(s) => &s.counters,
        ScenarioInfo::Roofline(r) => &r.counters,
    };

    let str_events = events
        .iter()
        .map(|evt| format!("{} INTEGER DEFAULT 0", evt))
        .collect::<Vec<_>>()
        .join(", ");

    connection.execute(format!(
        "
            CREATE TABLE pmu_counters (
                unique_id BINARY(128),
                process_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                time_enabled INTEGER NOT NULL,
                time_running INTEGER NOT NULL,
                confidence REAL NOT NULL,
                ip INTEGER NOT NULL,
                call_stack TEXT,
                {}
            );
        ",
        str_events
    ))?;

    let file = File::open(res_dir.join("events.bin"))
        .await
        .expect("failed to open events.bin");

    let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
    map.advise(Advice::Sequential)
        .expect("Failed to advice sequential reads");

    let proc_map_file = std::fs::File::open(res_dir.join("proc_map.json"))?;
    let proc_map: Vec<ProcMap> = serde_json::from_reader(proc_map_file)?;

    let resolved_pm = utils::resolve_proc_maps(&proc_map);

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    let mut counters = HashMap::<String, u64>::new();
    let mut lead_event: Option<Event> = None;

    let mut proc_map_stmt = connection
        .prepare("INSERT INTO proc_map (ip, func_name, file_name, line) VALUES (?, ?, ?, ?);")?;

    let mut known_ips = HashSet::<u64>::new();

    while (cursor.position() as usize) < map.len() {
        let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

        if !evt.ty.is_pmu() && !evt.ty.is_os() {
            continue;
        }

        let pm = resolved_pm.get(&evt.process_id);
        if pm.is_none() {
            continue;
        }
        let pm = pm.unwrap();

        if evt.correlation_id
            != lead_event
                .as_ref()
                .map(|e| e.correlation_id)
                .unwrap_or_default()
        {
            if !counters.is_empty() {
                let mut keys = vec![];
                let mut values = vec![];
                for (k, v) in counters.iter() {
                    keys.push(k.clone());
                    values.push(v.to_string());
                }

                let lead_event = lead_event.as_ref().unwrap();

                connection.execute(format!(
                    "
                        INSERT INTO pmu_counters (
                            unique_id,
                            process_id,
                            thread_id,
                            time_enabled,
                            time_running,
                            confidence,
                            ip,
                            call_stack,
                            {}
                        )

                        VALUES (
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          \"[{}]\",
                          {}
                        );
                    ",
                    keys.join(", "),
                    lead_event.unique_id,
                    lead_event.process_id,
                    lead_event.thread_id,
                    lead_event.time_enabled,
                    lead_event.time_running,
                    lead_event.time_running as f64 / lead_event.time_enabled as f64,
                    lead_event.callstack.first().unwrap().as_ip(),
                    lead_event
                        .callstack
                        .iter()
                        .map(|f| f.as_ip().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    values.join(", "),
                ))?;

                counters.clear();
            }

            lead_event = Some(evt.clone());

            for frame in evt.callstack {
                match frame {
                    CallFrame::Location(_) => unreachable!(),
                    CallFrame::IP(ip) => {
                        if known_ips.contains(&ip) {
                            continue;
                        }

                        known_ips.insert(ip);

                        proc_map_stmt.reset()?;
                        let sym_name = utils::find_sym_name(pm, ip as usize)
                            .unwrap_or("[unknown]".to_string());
                        let (file, line) = utils::find_location(pm, ip as usize)
                            .unwrap_or(("unknown".to_string(), 0));
                        proc_map_stmt.bind((1, ip as i64))?;
                        proc_map_stmt.bind((2, sym_name.as_str()))?;
                        proc_map_stmt.bind((3, file.as_str()))?;
                        proc_map_stmt.bind((4, line as i64))?;
                        proc_map_stmt.next()?;
                    }
                }
            }
        }

        counters.insert(format!("{}", evt.ty), evt.value);
    }

    Ok(())
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

async fn process_roofline_events(
    connection: &sqlite::Connection,
    info: &ScenarioInfo,
    res_dir: &Path,
) -> Result<()> {
    let (baseline_pid, instr_pid) = match info {
        ScenarioInfo::Roofline(roofline) => (roofline.perf_pid, roofline.inst_pid),
        _ => unimplemented!(),
    };

    connection.execute(
        "
            CREATE TABLE roofline_ops(
                unique_id BINARY(128),
                process_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                file_name BINARY(128) NOT NULL,
                function_name BINARY(128) NOT NULL,
                line INTEGER NOT NULL,
                bytes_load INTEGER NOT NULL,
                bytes_store INTEGER NOT NULL,
                scalar_int_ops INTEGER NOT NULL,
                scalar_float_ops INTEGER NOT NULL,
                scalar_double_ops INTEGER NOT NULL,
                vector_int_ops INTEGER NOT NULL,
                vector_float_ops INTEGER NOT NULL,
                vector_double_ops INTEGER NOT NULL
            );
        ",
    )?;
    connection.execute(
        "
            CREATE TABLE roofline_loop_runs(
                unique_id BINARY(128),
                process_id INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                file_name BINARY(128) NOT NULL,
                function_name BINARY(128) NOT NULL,
                line INTEGER NOT NULL,
                loop_start_ts INTEGER NOT NULL,
                loop_end_ts INTEGER NOT NULL
            );
        ",
    )?;

    let file = File::open(res_dir.join("events.bin"))
        .await
        .expect("failed to open events.bin");

    let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
    map.advise(Advice::Sequential)
        .expect("Failed to advice sequential reads");

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    #[derive(Default)]
    struct LoopInfo {
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

    let mut loops = HashMap::<u128, LoopInfo>::new();

    while (cursor.position() as usize) < map.len() {
        let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

        if !evt.ty.is_roofline() {
            continue;
        }

        match evt.ty {
            EventType::RooflineLoopStart => {
                let mut loop_info = LoopInfo::default();
                loop_info.id = evt.unique_id;
                loop_info.pid = evt.process_id;
                loop_info.tid = evt.thread_id;
                loop_info.file_name = evt.callstack[0].as_loc().file_name;
                loop_info.func_name = evt.callstack[0].as_loc().function_name;
                loop_info.line = evt.callstack[0].as_loc().line;
                loop_info.start = evt.timestamp;
                loops.insert(evt.unique_id, loop_info);
            }
            EventType::RooflineLoopEnd => {
                let loop_info = loops.remove(&evt.correlation_id).unwrap();

                if evt.process_id as i32 == baseline_pid {
                    connection.execute(format!(
                        "
                        INSERT INTO roofline_loop_runs (
                            unique_id,
                            process_id,
                            thread_id,
                            file_name,
                            function_name,
                            line,
                            loop_start_ts,
                            loop_end_ts
                        )

                        VALUES (
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {}
                        );
                    ",
                        loop_info.id,
                        loop_info.pid,
                        loop_info.tid,
                        loop_info.file_name,
                        loop_info.func_name,
                        loop_info.line,
                        loop_info.start,
                        evt.timestamp
                    ))?;
                } else if evt.process_id as i32 == instr_pid {
                    connection.execute(format!(
                        "
                        INSERT INTO roofline_ops (
                            unique_id,
                            process_id,
                            thread_id,
                            file_name,
                            function_name,
                            line,
                            bytes_load,
                            bytes_store,
                            scalar_int_ops,
                            scalar_float_ops,
                            scalar_double_ops,
                            vector_int_ops,
                            vector_float_ops,
                            vector_double_ops
                        )

                        VALUES (
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {},
                          {}
                        );
                    ",
                        loop_info.id,
                        loop_info.pid,
                        loop_info.tid,
                        loop_info.file_name,
                        loop_info.func_name,
                        loop_info.line,
                        loop_info.bytes_load,
                        loop_info.bytes_store,
                        loop_info.scalar_int_ops,
                        loop_info.scalar_float_ops,
                        loop_info.scalar_double_ops,
                        loop_info.vector_int_ops,
                        loop_info.vector_float_ops,
                        loop_info.vector_double_ops
                    ))?;
                }
            }
            EventType::RooflineBytesLoad => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.bytes_load = evt.value;
            }
            EventType::RooflineBytesStore => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.bytes_store = evt.value;
            }
            EventType::RooflineScalarIntOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.scalar_int_ops = evt.value;
            }
            EventType::RooflineScalarFloatOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.scalar_float_ops = evt.value;
            }
            EventType::RooflineScalarDoubleOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.scalar_double_ops = evt.value;
            }
            EventType::RooflineVectorIntOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.vector_int_ops = evt.value;
            }
            EventType::RooflineVectorFloatOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.vector_float_ops = evt.value;
            }
            EventType::RooflineVectorDoubleOps => {
                let loop_info = loops.get_mut(&evt.parent_id).unwrap();
                loop_info.vector_double_ops = evt.value;
            }
            _ => {}
        }
    }

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
  runs.process_id,
  s_file.string AS file_name,
  s_func.string AS function_name,
  runs.line,

  CAST(ops.scalar_int_ops AS REAL) / NULLIF(runs.total_duration, 0) AS scalar_int_ops,
  CAST(ops.scalar_int_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_int_ai,

  CAST(ops.scalar_float_ops AS REAL) / NULLIF(runs.total_duration, 0) AS scalar_float_ops,
  CAST(ops.scalar_float_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_float_ai,

  CAST(ops.scalar_double_ops AS REAL) / NULLIF(runs.total_duration, 0) AS scalar_double_ops,
  CAST(ops.scalar_double_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS scalar_double_ai,

  CAST(ops.vector_int_ops AS REAL) / NULLIF(runs.total_duration, 0) AS vector_int_ops,
  CAST(ops.vector_int_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_int_ai,

  CAST(ops.vector_float_ops AS REAL) / NULLIF(runs.total_duration, 0) AS vector_float_ops,
  CAST(ops.vector_float_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_float_ai,

  CAST(ops.vector_double_ops AS REAL) / NULLIF(runs.total_duration, 0) AS vector_double_ops,
  CAST(ops.vector_double_ops AS REAL) / NULLIF(ops.bytes_load + ops.bytes_store, 0) AS vector_double_ai

FROM runs
LEFT JOIN ops
  ON runs.process_id = ops.process_id
  AND runs.file_name = ops.file_name
  AND runs.function_name = ops.function_name
  AND runs.line = ops.line
LEFT JOIN strings s_file ON runs.file_name = s_file.id
LEFT JOIN strings s_func ON runs.function_name = s_func.id;
    ").expect("failed to create a view");
    Ok(())
}
