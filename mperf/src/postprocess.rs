use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::Result;
use kdam::BarExt;
use memmap2::{Advice, Mmap};
use mperf_data::{
    CallFrame, Event, EventType, IString, ProcMapEntry, RecordInfo, Scenario, ScenarioInfo,
};
use smallvec::SmallVec;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

use crate::disassembly::{default_disassembler, DisassembleRequest};
use crate::utils;

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
            process_roofline_events(&connection, &info.scenario_info, res_dir, &mut pb).await?;
            process_disassembly(&connection, res_dir, &mut pb).await?;
            create_hotspots_view(&connection).await?;
            create_roofline_view(&connection).await?;
        }
        Scenario::TMA => {
            process_pmu_counters(&connection, &info.scenario_info, res_dir, &mut pb).await?;
            create_tma_view(&connection, &info.scenario_info).await?;
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

fn get_event_column_name(evt: &(EventType, String)) -> String {
    match evt.0 {
        EventType::PmuCustom => format!("pmu_{}", evt.1.replace(".", "_")),
        _ => evt.0.to_string(),
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

    let str_events = events
        .iter()
        .map(|evt| format!("{} INTEGER DEFAULT 0", get_event_column_name(evt)))
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

    pb.reset(Some(map.len()));
    pb.write("Coolecting hotspots")?;

    let strings_file =
        std::fs::File::open(res_dir.join("strings.json")).expect("failed to open strings.json");
    let strings: Vec<IString> =
        serde_json::from_reader(strings_file).expect("failed to parse strings.json");

    let find_string = |str_id| {
        if str_id == 0 {
            return String::new();
        }

        strings
            .iter()
            .find_map(|istr| {
                if istr.id == str_id {
                    Some(istr.value.clone())
                } else {
                    None
                }
            })
            .unwrap()
    };

    let proc_map_file = std::fs::File::open(res_dir.join("proc_map.json"))?;
    let proc_map: Vec<ProcMapEntry> = serde_json::from_reader(proc_map_file)?;

    let resolved_pm = utils::resolve_proc_maps(&proc_map);

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    let mut counters = HashMap::<String, u64>::new();
    let mut lead_event: Option<Event> = None;

    let mut proc_map_stmt = connection.prepare(
        "INSERT INTO proc_map (ip, func_name, file_name, line, module_path) VALUES (?, ?, ?, ?, ?);",
    )?;

    let mut known_ips = HashSet::<u64>::new();

    let mut flamegraph_cycles = HashMap::<String, u64>::new();
    let mut flamegraph_instructions = HashMap::<String, u64>::new();

    while (cursor.position() as usize) < map.len() {
        let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

        pb.update_to(cursor.position() as usize)?;

        if !evt.ty.is_pmu() && !evt.ty.is_os() {
            continue;
        }

        let pm = resolved_pm.get(&evt.process_id);

        if pm.is_none() {
            continue;
        }
        let pm = pm.unwrap();

        let func_names = evt
            .callstack
            .iter()
            .rev()
            .map(|frame| match frame {
                CallFrame::Location(_) => unreachable!(),
                CallFrame::IP(ip) => {
                    utils::find_sym_name(pm, *ip as usize).unwrap_or("[unknown]".to_string())
                }
            })
            .collect::<SmallVec<[_; 32]>>()
            .join(";");

        if evt.ty == EventType::PmuCycles {
            *flamegraph_cycles.entry(func_names).or_default() += evt.value;
        } else if evt.ty == EventType::PmuInstructions {
            *flamegraph_instructions.entry(func_names).or_default() += evt.value;
        }

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
                        let module_path = utils::find_module_path(pm, ip as usize);
                        proc_map_stmt.bind((1, ip as i64))?;
                        proc_map_stmt.bind((2, sym_name.as_str()))?;
                        proc_map_stmt.bind((3, file.as_str()))?;
                        proc_map_stmt.bind((4, line as i64))?;
                        proc_map_stmt.bind((5, module_path.as_deref()))?;
                        proc_map_stmt.next()?;
                    }
                }
            }
        }

        counters.insert(
            get_event_column_name(&(evt.ty, find_string(evt.name))),
            evt.value,
        );
    }

    let flamegraph_cycles = flamegraph_cycles
        .into_iter()
        .map(|(key, value)| format!("{} {}", key, value))
        .collect::<Vec<_>>();
    let flamegraph_instructions = flamegraph_instructions
        .into_iter()
        .map(|(key, value)| format!("{} {}", key, value))
        .collect::<Vec<_>>();

    let mut options = inferno::flamegraph::Options::default();
    options.reverse_stack_order = false;
    let fg_file = std::fs::File::create(res_dir.join("flamegraph_cycles.svg"))?;
    inferno::flamegraph::from_lines(
        &mut options,
        flamegraph_cycles.iter().map(|s| s.as_str()),
        &fg_file,
    )?;
    let fg_file = std::fs::File::create(res_dir.join("flamegraph_instructions.svg"))?;
    inferno::flamegraph::from_lines(
        &mut options,
        flamegraph_instructions.iter().map(|s| s.as_str()),
        &fg_file,
    )?;

    let mut fg_file = File::create(res_dir.join("flamegraph_cycles.folded")).await?;

    for fc in flamegraph_cycles {
        fg_file.write_all(fc.as_bytes()).await?;
        fg_file.write_all("\n".as_bytes()).await?;
    }

    let mut fg_file = File::create(res_dir.join("flamegraph_instructions.folded")).await?;

    for fi in flamegraph_instructions {
        fg_file.write_all(fi.as_bytes()).await?;
        fg_file.write_all("\n".as_bytes()).await?;
    }

    Ok(())
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
        "CREATE INDEX IF NOT EXISTS idx_assembly_module_rel_address ON assembly_lines(module_path, rel_address);",
    )?;

    connection.execute(
        "CREATE TABLE IF NOT EXISTS assembly_module_metadata (
            module_path TEXT PRIMARY KEY,
            load_bias INTEGER NOT NULL
        );",
    )?;

    let mut module_stmt = connection.prepare(
        "SELECT DISTINCT module_path FROM proc_map WHERE module_path IS NOT NULL AND module_path <> '';",
    )?;
    let mut modules = Vec::new();
    while let State::Row = module_stmt.next()? {
        modules.push(module_stmt.read::<String, _>(0)?);
    }

    if modules.is_empty() {
        return Ok(());
    }

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

    pb.reset(Some(modules.len()));
    pb.write("Extracting assembly")?;

    let mut delete_stmt =
        connection.prepare("DELETE FROM assembly_lines WHERE module_path = ?;")?;
    let mut insert_stmt = connection.prepare(
        "INSERT INTO assembly_lines (module_path, symbol, rel_address, runtime_address, instruction, source_file, source_line)
         VALUES (?, ?, ?, ?, ?, ?, ?);",
    )?;
    let mut metadata_stmt = connection.prepare(
        "INSERT INTO assembly_module_metadata (module_path, load_bias)
         VALUES (?, ?)
         ON CONFLICT(module_path) DO UPDATE SET load_bias = excluded.load_bias;",
    )?;

    connection.execute("BEGIN IMMEDIATE TRANSACTION;")?;

    for (idx, module_path) in modules.iter().enumerate() {
        pb.update_to(idx + 1)?;

        let module_file = Path::new(module_path);
        if !module_file.exists() {
            continue;
        }

        let load_bias = module_bias.get(module_path).copied().unwrap_or(0);

        metadata_stmt.reset()?;
        metadata_stmt.bind((1, module_path.as_str()))?;
        metadata_stmt.bind((2, load_bias))?;
        let _ = metadata_stmt.next()?;

        delete_stmt.reset()?;
        delete_stmt.bind((1, module_path.as_str()))?;
        let _ = delete_stmt.next()?;

        let request = DisassembleRequest {
            module_path: module_file.to_path_buf(),
            load_bias,
        };

        let lines = match disassembler.disassemble(&request) {
            Ok(lines) => lines,
            Err(err) => {
                eprintln!("failed to disassemble {}: {err}", module_path);
                continue;
            }
        };

        if lines.is_empty() {
            continue;
        }

        let bias_u64 = if load_bias >= 0 { load_bias as u64 } else { 0 };
        let bias_abs = if load_bias < 0 {
            (-load_bias) as u64
        } else {
            0
        };

        let use_bias_as_base =
            bias_u64 != 0 && lines.iter().all(|line| line.rel_address >= bias_u64);
        let rel_base = if use_bias_as_base { bias_u64 } else { 0 };

        for line in lines {
            insert_stmt.reset()?;
            insert_stmt.bind((1, module_path.as_str()))?;
            insert_stmt.bind((2, line.symbol.as_deref()))?;
            let rel_address = line.rel_address.saturating_sub(rel_base);
            let runtime_address = if load_bias >= 0 {
                bias_u64.saturating_add(rel_address)
            } else {
                if rel_address < bias_abs {
                    continue;
                }
                rel_address - bias_abs
            };
            insert_stmt.bind((3, rel_address as i64))?;
            insert_stmt.bind((4, runtime_address as i64))?;
            insert_stmt.bind((5, line.instruction.as_str()))?;
            insert_stmt.bind((6, line.source_file.as_deref()))?;
            insert_stmt.bind((7, line.source_line.map(|v| v as i64)))?;
            let _ = insert_stmt.next()?;
        }
    }

    connection.execute("COMMIT;")?;

    connection.execute("DROP VIEW IF EXISTS assembly_address_stats;")?;
    connection.execute(
        "
        CREATE VIEW assembly_address_stats AS
        SELECT
            module_path,
            func_name,
            address,
            SUM(samples) AS samples,
            SUM(cycles) AS cycles,
            SUM(instructions) AS instructions,
            SUM(branch_misses) AS branch_misses,
            SUM(branch_instructions) AS branch_instructions,
            SUM(llc_misses) AS llc_misses,
            SUM(llc_references) AS llc_references
        FROM assembly_samples
        GROUP BY module_path, func_name, address;
        ",
    )?;

    Ok(())
}

fn populate_assembly_samples(connection: &sqlite::Connection) -> Result<()> {
    use sqlite::State;

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

    connection.execute("DELETE FROM assembly_samples;")?;

    let mut select_stmt = connection.prepare("SELECT * FROM pmu_counters;")?;

    let mut lookup_stmt =
        connection.prepare("SELECT module_path, func_name FROM proc_map WHERE ip = ? LIMIT 1;")?;

    let mut insert_stmt = connection.prepare(
        "INSERT INTO assembly_samples (
            module_path,
            func_name,
            address,
            samples,
            cycles,
            instructions,
            branch_misses,
            branch_instructions,
            llc_misses,
            llc_references
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(module_path, func_name, address) DO UPDATE SET
            samples = samples + excluded.samples,
            cycles = cycles + excluded.cycles,
            instructions = instructions + excluded.instructions,
            branch_misses = branch_misses + excluded.branch_misses,
            branch_instructions = branch_instructions + excluded.branch_instructions,
            llc_misses = llc_misses + excluded.llc_misses,
            llc_references = llc_references + excluded.llc_references;",
    )?;

    while let State::Row = select_stmt.next()? {
        let ip = select_stmt.read::<i64, _>("ip")? as u64;

        lookup_stmt.reset()?;
        lookup_stmt.bind((1, ip as i64))?;

        match lookup_stmt.next()? {
            State::Row => {
                let module_path = lookup_stmt.read::<Option<String>, _>(0)?;
                let func_name = lookup_stmt.read::<Option<String>, _>(1)?;

                let module_path = match module_path {
                    Some(path) if !path.is_empty() => path,
                    _ => continue,
                };

                let func_name = func_name.unwrap_or_else(|| "[unknown]".to_string());

                let cycles = read_metric(&select_stmt, "pmu_cycles")?;
                let instructions = read_metric(&select_stmt, "pmu_instructions")?;
                let branch_misses = read_metric(&select_stmt, "pmu_branch_misses")?;
                let branch_instructions = read_metric(&select_stmt, "pmu_branch_instructions")?;
                let llc_misses = read_metric(&select_stmt, "pmu_llc_misses")?;
                let llc_references = read_metric(&select_stmt, "pmu_llc_references")?;

                insert_stmt.reset()?;
                insert_stmt.bind((1, module_path.as_str()))?;
                insert_stmt.bind((2, func_name.as_str()))?;
                insert_stmt.bind((3, ip as i64))?;
                insert_stmt.bind((4, 1_i64))?;
                insert_stmt.bind((5, cycles))?;
                insert_stmt.bind((6, instructions))?;
                insert_stmt.bind((7, branch_misses))?;
                insert_stmt.bind((8, branch_instructions))?;
                insert_stmt.bind((9, llc_misses))?;
                insert_stmt.bind((10, llc_references))?;
                let _ = insert_stmt.next()?;
            }
            State::Done => {}
        }
    }

    Ok(())
}

fn read_metric(stmt: &sqlite::Statement<'_>, name: &str) -> Result<i64> {
    for idx in 0..stmt.column_count() {
        if stmt.column_name(idx).unwrap_or("") == name {
            let value = stmt.read::<Option<i64>, _>(idx)?;
            return Ok(value.unwrap_or(0));
        }
    }
    Ok(0)
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
    pb: &mut kdam::Bar,
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

    pb.reset(Some(map.len()));
    pb.write("Coolecting roofline data")?;

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

        pb.update_to(cursor.position() as usize)?;

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

async fn create_tma_view(connection: &sqlite::Connection, info: &ScenarioInfo) -> Result<()> {
    let info = match info {
        ScenarioInfo::TMA(tma) => tma,
        _ => unreachable!(),
    };

    let mut cols = vec![];
    for metric in &info.metrics {
        let expr = pmu_data::arith_parser::parse_expr(&metric.formula);
        let sql = build_sql_expr(&info.metrics, &info.counters, &info.constants, &expr);
        cols.push(format!("{} AS {}", sql, metric.name.replace(".", "_")));
    }

    connection
        .execute(format!(
            "
    CREATE VIEW tma
    AS
    SELECT
        proc_map.func_name as func_name,
        COUNT(pmu_counters.pmu_cycles) AS num_samples,
        (SUM(pmu_counters.pmu_cycles) * 1.0 / (SELECT SUM(pmu_cycles) FROM pmu_counters)) AS total,
        SUM(pmu_counters.pmu_cycles) AS cycles,
        SUM(pmu_counters.pmu_instructions) AS instructions,
        (SUM(pmu_counters.pmu_instructions) * 1.0 / SUM(pmu_counters.pmu_cycles)) AS ipc,
        {}
    FROM pmu_counters
    INNER JOIN proc_map ON pmu_counters.ip = proc_map.ip
    GROUP BY proc_map.func_name;
    ",
            cols.join(",\n")
        ))
        .expect("failed to create a view");

    Ok(())
}

fn build_sql_expr(
    metrics: &[pmu_data::Metric],
    events: &[(EventType, String)],
    constants: &[pmu_data::Constant],
    expr: &pmu_data::arith_parser::Expr,
) -> String {
    match expr {
        pmu_data::arith_parser::Expr::Variable(var) => events
            .iter()
            .find_map(|(ty, name)| {
                if name == var {
                    if ty == &EventType::PmuCycles || ty == &EventType::PmuInstructions {
                        // Cycles and instructions are fixed counters and do not require scaling
                        Some(format!(
                            "SUM(pmu_counters.{})",
                            get_event_column_name(&(*ty, name.clone()))
                        ))
                    } else {
                        Some(format!(
                            "SUM(pmu_counters.{} / pmu_counters.confidence)",
                            get_event_column_name(&(*ty, name.clone()))
                        ))
                    }
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                let new_expr = metrics
                    .iter()
                    .find_map(|m| {
                        if m.name.as_str() == var {
                            let expr = pmu_data::arith_parser::parse_expr(&m.formula);
                            Some(build_sql_expr(metrics, events, constants, &expr))
                        } else {
                            None
                        }
                    })
                    .unwrap();

                format!("({})", new_expr)
            }),
        pmu_data::arith_parser::Expr::Constant(cst_name) => {
            let value = constants
                .iter()
                .find_map(|cst| {
                    if cst.name.as_str() == cst_name {
                        Some(cst.value)
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            format!("{}", value)
        }
        pmu_data::arith_parser::Expr::Binary { op, lhs, rhs } => {
            let op_str = match op {
                pmu_data::arith_parser::BinOp::Add => "+",
                pmu_data::arith_parser::BinOp::Sub => "-",
                pmu_data::arith_parser::BinOp::Mul => "*",
                pmu_data::arith_parser::BinOp::Div => "/",
            };

            let lhs_str = build_sql_expr(metrics, events, constants, lhs);
            let rhs_str = build_sql_expr(metrics, events, constants, rhs);

            match op {
                pmu_data::arith_parser::BinOp::Div => {
                    format!("CAST(({}) AS REAL) / CAST(({}) AS REAL)", lhs_str, rhs_str)
                }
                _ => format!("({}) {} ({})", lhs_str, op_str, rhs_str),
            }
        }
        pmu_data::arith_parser::Expr::Num(num) => num.to_string(),
    }
}
