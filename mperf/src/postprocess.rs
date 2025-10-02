use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    path::Path,
};

use anyhow::{Context, Result};
use kdam::BarExt;
use memmap2::{Advice, Mmap};
use mperf_data::{
    CallFrame, Event, EventType, IString, ProcMapEntry, RecordInfo, Scenario, ScenarioInfo,
};
use smallvec::SmallVec;
use sqlite::{State, Value};
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
    }

    Ok(())
}

struct LeadRecord {
    unique_id: u128,
    process_id: u32,
    thread_id: u32,
    time_enabled: u64,
    time_running: u64,
    first_ip: u64,
    callstack_ips: SmallVec<[u64; 32]>,
}

impl LeadRecord {
    fn from_event(evt: &Event) -> Self {
        let callstack_ips = evt
            .callstack
            .iter()
            .filter_map(|frame| match frame {
                CallFrame::Location(_) => None,
                CallFrame::IP(ip) => Some(*ip),
            })
            .collect::<SmallVec<[u64; 32]>>();

        let first_ip = callstack_ips.first().copied().unwrap_or_default();

        Self {
            unique_id: evt.unique_id,
            process_id: evt.process_id,
            thread_id: evt.thread_id,
            time_enabled: evt.time_enabled,
            time_running: evt.time_running,
            first_ip,
            callstack_ips,
        }
    }

    fn call_stack_string(&self) -> String {
        if self.callstack_ips.is_empty() {
            return "[]".to_string();
        }

        let joined = self
            .callstack_ips
            .iter()
            .map(|ip| ip.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("[{}]", joined)
    }
}

fn sqlite_i64_from_u64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn sqlite_u64_from_i64(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

async fn process_strings(connection: &sqlite::Connection, res_dir: &Path) -> Result<()> {
    let strings_file =
        std::fs::File::open(res_dir.join("strings.json")).expect("failed to open strings.json");
    let strings: Vec<IString> =
        serde_json::from_reader(strings_file).expect("failed to parse strings.json");

    connection.execute("BEGIN TRANSACTION;")?;
    let mut stmt = connection.prepare("INSERT INTO strings (id, string) VALUES (?, ?);")?;

    for s in strings {
        stmt.reset()?;
        let values = [
            Value::Binary(s.id.to_le_bytes().to_vec()),
            Value::String(s.value),
        ];
        stmt.bind(values.as_slice())?;
        let state = stmt.next()?;
        debug_assert!(matches!(state, State::Done));
    }

    connection.execute("COMMIT;")?;

    Ok(())
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

    pb.reset(Some(map.len()));
    pb.write("Coolecting hotspots")?;

    let proc_map_file = std::fs::File::open(res_dir.join("proc_map.json"))?;
    let proc_map: Vec<ProcMapEntry> = serde_json::from_reader(proc_map_file)?;

    let resolved_pm = utils::resolve_proc_maps(&proc_map);

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    let mut lead_event: Option<LeadRecord> = None;

    let mut proc_map_stmt = connection.prepare(
        "INSERT INTO proc_map (ip, func_name, file_name, line, module_path) VALUES (?, ?, ?, ?, ?);",
    )?;

    let mut known_ips = HashSet::<u64>::new();

    let mut flamegraph_cycles = HashMap::<String, u64>::new();
    let mut flamegraph_instructions = HashMap::<String, u64>::new();

    let event_columns: Vec<EventType> = events.clone();
    let event_column_names = event_columns
        .iter()
        .map(|evt| evt.to_string())
        .collect::<Vec<_>>();
    let column_index = event_columns
        .iter()
        .enumerate()
        .map(|(idx, evt)| (*evt, idx))
        .collect::<HashMap<_, _>>();

    let mut column_names = vec![
        "unique_id".to_string(),
        "process_id".to_string(),
        "thread_id".to_string(),
        "time_enabled".to_string(),
        "time_running".to_string(),
        "confidence".to_string(),
        "ip".to_string(),
        "call_stack".to_string(),
    ];
    column_names.extend(event_column_names.clone());
    let placeholder_list = (0..column_names.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO pmu_counters ({}) VALUES ({});",
        column_names.join(", "),
        placeholder_list
    );
    let mut counters = vec![0_u64; event_columns.len()];

    connection.execute("BEGIN TRANSACTION;")?;
    let mut insert_stmt = connection.prepare(insert_sql)?;

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

        if evt.correlation_id != lead_event.as_ref().map(|e| e.unique_id).unwrap_or_default() {
            if let Some(lead) = &lead_event {
                insert_stmt.reset()?;
                flush_lead_record(&mut insert_stmt, lead, &counters)?;
                for value in counters.iter_mut() {
                    *value = 0;
                }
            }

            let lead = LeadRecord::from_event(&evt);
            for frame in evt.callstack.iter() {
                if let CallFrame::IP(ip) = frame {
                    if known_ips.insert(*ip) {
                        proc_map_stmt.reset()?;
                        let sym_name = utils::find_sym_name(pm, *ip as usize)
                            .unwrap_or("[unknown]".to_string());
                        let (file, line) = utils::find_location(pm, *ip as usize)
                            .unwrap_or(("unknown".to_string(), 0));
                        let module_path = utils::find_module_path(pm, *ip as usize);
                        proc_map_stmt.bind((1, sqlite_i64_from_u64(*ip)))?;
                        proc_map_stmt.bind((2, sym_name.as_str()))?;
                        proc_map_stmt.bind((3, file.as_str()))?;
                        proc_map_stmt.bind((4, line as i64))?;
                        proc_map_stmt.bind((5, module_path.as_deref()))?;
                        proc_map_stmt.next()?;
                    }
                }
            }

            lead_event = Some(lead);
        }

        if let Some(idx) = column_index.get(&evt.ty) {
            counters[*idx] = evt.value;
        }
    }

    if let Some(lead) = &lead_event {
        insert_stmt.reset()?;
        flush_lead_record(&mut insert_stmt, lead, &counters)?;
    }

    connection.execute("COMMIT;")?;

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
            insert_stmt.bind((3, sqlite_i64_from_u64(rel_address)))?;
            insert_stmt.bind((4, sqlite_i64_from_u64(runtime_address)))?;
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
        let ip = sqlite_u64_from_i64(select_stmt.read::<i64, _>("ip")?);

        lookup_stmt.reset()?;
        lookup_stmt.bind((1, sqlite_i64_from_u64(ip)))?;

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
                insert_stmt.bind((3, sqlite_i64_from_u64(ip)))?;
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

fn flush_lead_record(
    insert_stmt: &mut sqlite::Statement,
    lead: &LeadRecord,
    counters: &[u64],
) -> Result<()> {
    let mut values = Vec::with_capacity(8 + counters.len());
    values.push(Value::Binary(lead.unique_id.to_le_bytes().to_vec()));
    values.push(Value::Integer(i64::from(lead.process_id))); // u32 -> i64
    values.push(Value::Integer(i64::from(lead.thread_id)));
    values.push(Value::Integer(
        i64::try_from(lead.time_enabled).context("time_enabled overflowed i64")?,
    ));
    values.push(Value::Integer(
        i64::try_from(lead.time_running).context("time_running overflowed i64")?,
    ));

    let confidence = if lead.time_enabled == 0 {
        0.0
    } else {
        lead.time_running as f64 / lead.time_enabled as f64
    };
    values.push(Value::Float(confidence));
    values.push(Value::Integer(sqlite_i64_from_u64(lead.first_ip)));
    values.push(Value::String(lead.call_stack_string()));

    for value in counters {
        values.push(Value::Integer(
            i64::try_from(*value).context("counter overflowed i64")?,
        ));
    }

    insert_stmt.bind(values.as_slice())?;
    let state = insert_stmt.next()?;
    debug_assert!(matches!(state, State::Done));
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

    connection.execute("BEGIN TRANSACTION;")?;

    let mut loop_runs_stmt = connection.prepare(
        "INSERT INTO roofline_loop_runs (
            unique_id,
            process_id,
            thread_id,
            file_name,
            function_name,
            line,
            loop_start_ts,
            loop_end_ts
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
    )?;

    let mut loop_ops_stmt = connection.prepare(
        "INSERT INTO roofline_ops (
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
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
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
                    loop_runs_stmt.reset()?;
                    let values = [
                        Value::Binary(loop_info.id.to_le_bytes().to_vec()),
                        Value::Integer(i64::from(loop_info.pid)),
                        Value::Integer(i64::from(loop_info.tid)),
                        Value::Binary(loop_info.file_name.to_le_bytes().to_vec()),
                        Value::Binary(loop_info.func_name.to_le_bytes().to_vec()),
                        Value::Integer(i64::from(loop_info.line)),
                        Value::Integer(
                            i64::try_from(loop_info.start).context("loop start overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(evt.timestamp).context("loop end overflowed i64")?,
                        ),
                    ];
                    loop_runs_stmt.bind(values.as_slice())?;
                    let state = loop_runs_stmt.next()?;
                    debug_assert!(matches!(state, State::Done));
                } else if evt.process_id as i32 == instr_pid {
                    loop_ops_stmt.reset()?;
                    let values = [
                        Value::Binary(loop_info.id.to_le_bytes().to_vec()),
                        Value::Integer(i64::from(loop_info.pid)),
                        Value::Integer(i64::from(loop_info.tid)),
                        Value::Binary(loop_info.file_name.to_le_bytes().to_vec()),
                        Value::Binary(loop_info.func_name.to_le_bytes().to_vec()),
                        Value::Integer(i64::from(loop_info.line)),
                        Value::Integer(
                            i64::try_from(loop_info.bytes_load)
                                .context("bytes_load overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.bytes_store)
                                .context("bytes_store overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.scalar_int_ops)
                                .context("scalar_int_ops overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.scalar_float_ops)
                                .context("scalar_float_ops overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.scalar_double_ops)
                                .context("scalar_double_ops overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.vector_int_ops)
                                .context("vector_int_ops overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.vector_float_ops)
                                .context("vector_float_ops overflowed i64")?,
                        ),
                        Value::Integer(
                            i64::try_from(loop_info.vector_double_ops)
                                .context("vector_double_ops overflowed i64")?,
                        ),
                    ];
                    loop_ops_stmt.bind(values.as_slice())?;
                    let state = loop_ops_stmt.next()?;
                    debug_assert!(matches!(state, State::Done));
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

    connection.execute("COMMIT;")?;

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
mod tests {
    use super::*;
    use kdam::Bar;
    use mperf_data::{Location, RooflineInfo, SnapshotInfo};
    use smallvec::smallvec;
    use tempfile::tempdir;

    fn create_connection(dir: &Path) -> sqlite::Connection {
        sqlite::open(dir.join("perf.db")).expect("failed to open sqlite")
    }

    fn write_event<W: std::io::Write>(writer: &mut W, event: Event) -> Result<()> {
        event
            .write_binary(writer)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    #[tokio::test]
    async fn process_strings_inserts_rows() -> Result<()> {
        let dir = tempdir()?;
        let connection = create_connection(dir.path());
        connection
            .execute("CREATE TABLE strings (id BINARY(128) NOT NULL, string TEXT NOT NULL);")?;

        let strings = vec![
            IString {
                id: 1,
                value: "alpha".to_string(),
            },
            IString {
                id: 2,
                value: "beta".to_string(),
            },
        ];
        let file = std::fs::File::create(dir.path().join("strings.json"))?;
        serde_json::to_writer(file, &strings)?;

        process_strings(&connection, dir.path()).await?;

        let mut stmt = connection.prepare("SELECT COUNT(*) FROM strings;")?;
        assert!(matches!(stmt.next()?, State::Row));
        let count: i64 = stmt.read(0)?;
        assert_eq!(count, 2);

        Ok(())
    }

    #[tokio::test]
    async fn process_pmu_counters_inserts_rows() -> Result<()> {
        let dir = tempdir()?;
        let connection = create_connection(dir.path());
        connection.execute(
            "
            CREATE TABLE proc_map (ip INTEGER, func_name TEXT, file_name TEXT, line INTEGER, module_path TEXT);
            CREATE TABLE strings (id BINARY(128) NOT NULL, string TEXT NOT NULL);
        ",
        )?;

        let events_path = dir.path().join("events.bin");
        {
            let mut writer = std::io::BufWriter::new(std::fs::File::create(&events_path)?);
            write_event(
                &mut writer,
                Event {
                    unique_id: 1,
                    correlation_id: 1,
                    parent_id: 0,
                    ty: EventType::PmuCycles,
                    thread_id: 10,
                    process_id: 20,
                    time_enabled: 200,
                    time_running: 100,
                    value: 300,
                    timestamp: 1,
                    callstack: smallvec![CallFrame::IP(0x10)],
                },
            )?;
            write_event(
                &mut writer,
                Event {
                    unique_id: 2,
                    correlation_id: 1,
                    parent_id: 0,
                    ty: EventType::PmuInstructions,
                    thread_id: 10,
                    process_id: 20,
                    time_enabled: 200,
                    time_running: 100,
                    value: 400,
                    timestamp: 2,
                    callstack: smallvec![CallFrame::IP(0x10)],
                },
            )?;
        }

        let proc_map = vec![ProcMapEntry {
            filename: "nonexistent".to_string(),
            address: 0,
            size: 4096,
            offset: 0,
            pid: 20,
        }];
        let file = std::fs::File::create(dir.path().join("proc_map.json"))?;
        serde_json::to_writer(file, &proc_map)?;

        let info = ScenarioInfo::Snapshot(SnapshotInfo {
            pid: 20,
            counters: vec![EventType::PmuCycles, EventType::PmuInstructions],
        });

        let mut pb = Bar::new(0);
        process_pmu_counters(&connection, &info, dir.path(), &mut pb).await?;

        let mut stmt = connection
            .prepare("SELECT pmu_cycles, pmu_instructions, call_stack FROM pmu_counters;")?;
        assert!(matches!(stmt.next()?, State::Row));
        let cycles: i64 = stmt.read(0)?;
        let instructions: i64 = stmt.read(1)?;
        let call_stack: String = stmt.read(2)?;
        assert_eq!(cycles, 300);
        assert_eq!(instructions, 400);
        assert_eq!(call_stack, "[16]");

        Ok(())
    }

    #[tokio::test]
    async fn process_roofline_events_inserts_rows() -> Result<()> {
        let dir = tempdir()?;
        let connection = create_connection(dir.path());

        let baseline_pid = 30;
        let instr_pid = 31;
        let events_path = dir.path().join("events.bin");
        {
            let mut writer = std::io::BufWriter::new(std::fs::File::create(&events_path)?);

            write_event(
                &mut writer,
                Event {
                    unique_id: 100,
                    correlation_id: 0,
                    parent_id: 0,
                    ty: EventType::RooflineLoopStart,
                    thread_id: 1,
                    process_id: baseline_pid as u32,
                    time_enabled: 0,
                    time_running: 0,
                    value: 0,
                    timestamp: 10,
                    callstack: smallvec![CallFrame::Location(Location {
                        function_name: 1,
                        file_name: 2,
                        line: 7,
                    })],
                },
            )?;

            write_event(
                &mut writer,
                Event {
                    unique_id: 101,
                    correlation_id: 100,
                    parent_id: 0,
                    ty: EventType::RooflineLoopEnd,
                    thread_id: 1,
                    process_id: baseline_pid as u32,
                    time_enabled: 0,
                    time_running: 0,
                    value: 0,
                    timestamp: 30,
                    callstack: smallvec![],
                },
            )?;

            write_event(
                &mut writer,
                Event {
                    unique_id: 200,
                    correlation_id: 0,
                    parent_id: 0,
                    ty: EventType::RooflineLoopStart,
                    thread_id: 2,
                    process_id: instr_pid as u32,
                    time_enabled: 0,
                    time_running: 0,
                    value: 0,
                    timestamp: 40,
                    callstack: smallvec![CallFrame::Location(Location {
                        function_name: 1,
                        file_name: 2,
                        line: 7,
                    })],
                },
            )?;

            let mut stats_event = |ty: EventType, value: u64| -> Result<()> {
                write_event(
                    &mut writer,
                    Event {
                        unique_id: 201,
                        correlation_id: 0,
                        parent_id: 200,
                        ty,
                        thread_id: 2,
                        process_id: instr_pid as u32,
                        time_enabled: 0,
                        time_running: 0,
                        value,
                        timestamp: 45,
                        callstack: smallvec![],
                    },
                )
            };
            stats_event(EventType::RooflineBytesLoad, 10)?;
            stats_event(EventType::RooflineBytesStore, 20)?;
            stats_event(EventType::RooflineScalarIntOps, 30)?;
            stats_event(EventType::RooflineScalarFloatOps, 40)?;
            stats_event(EventType::RooflineScalarDoubleOps, 50)?;
            stats_event(EventType::RooflineVectorIntOps, 60)?;
            stats_event(EventType::RooflineVectorFloatOps, 65)?;
            stats_event(EventType::RooflineVectorDoubleOps, 70)?;

            write_event(
                &mut writer,
                Event {
                    unique_id: 202,
                    correlation_id: 200,
                    parent_id: 0,
                    ty: EventType::RooflineLoopEnd,
                    thread_id: 2,
                    process_id: instr_pid as u32,
                    time_enabled: 0,
                    time_running: 0,
                    value: 0,
                    timestamp: 80,
                    callstack: smallvec![],
                },
            )?;
        }

        let info = ScenarioInfo::Roofline(RooflineInfo {
            perf_pid: baseline_pid,
            counters: vec![],
            inst_pid: instr_pid,
        });

        let mut pb = Bar::new(0);
        process_roofline_events(&connection, &info, dir.path(), &mut pb).await?;

        let mut runs_stmt = connection.prepare("SELECT COUNT(*) FROM roofline_loop_runs;")?;
        assert!(matches!(runs_stmt.next()?, State::Row));
        let run_count: i64 = runs_stmt.read(0)?;
        assert_eq!(run_count, 1);

        let mut ops_stmt =
            connection.prepare("SELECT bytes_load, vector_double_ops FROM roofline_ops;")?;
        assert!(matches!(ops_stmt.next()?, State::Row));
        let bytes_load: i64 = ops_stmt.read(0)?;
        let vector_double_ops: i64 = ops_stmt.read(1)?;
        assert_eq!(bytes_load, 10);
        assert_eq!(vector_double_ops, 70);

        Ok(())
    }
}
