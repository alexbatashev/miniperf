use std::path::Path;

use anyhow::Result;
use mperf_data::{CallFrame, Event, EventType, Location, UserRegs};
use store::duckdb::types::Value;
use store::{EventKind, Session};

use crate::utils;

/// Print every recorded event of a session directory as JSON.
pub fn do_events_export(path: &Path) {
    match export(path) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("failed to export events: {err:#}"),
    }
}

fn export(path: &Path) -> Result<String> {
    let session = Session::open(path)?;
    let strings = utils::load_strings(session.connection())?;
    let mut events = Vec::new();

    // Postprocess consumes `samples_raw`; afterwards the resolved `samples`
    // table carries the same rows with their symbolized stacks.
    let sample_query = if session.has_table("samples_raw") {
        Some(
            "SELECT timestamp, pid, tid, cpu, group_id, event_id, value, time_enabled,
                    time_running, callchain, regs_abi, regs_mask, regs
             FROM samples_raw",
        )
    } else if session.has_table("samples") {
        Some(
            "SELECT s.timestamp, s.pid, s.tid, s.cpu, s.group_id, s.event_id, s.value,
                    s.time_enabled, s.time_running, k.frames,
                    CAST(0 AS UBIGINT), CAST(0 AS UBIGINT), CAST(NULL AS UBIGINT[])
             FROM samples s LEFT JOIN stacks k ON k.stack_id = s.stack_id",
        )
    } else {
        None
    };

    if let Some(sample_query) = sample_query {
        let mut statement = session.connection().prepare(sample_query)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let name: u64 = row.get(5)?;
            let event_name = strings.get(&name).map(String::as_str).unwrap_or("");
            let ty = utils::event_type_from_name(event_name).unwrap_or(EventType::PmuCustom);
            let regs_mask: u64 = row.get(11)?;
            events.push(Event {
                unique_id: 0,
                correlation_id: row.get(4)?,
                parent_id: 0,
                ty,
                thread_id: row.get(2)?,
                process_id: row.get(1)?,
                cpu: row.get(3)?,
                time_enabled: row.get(7)?,
                time_running: row.get(8)?,
                value: row.get::<_, i64>(6)? as u64,
                timestamp: row.get::<_, i64>(0)? as u64,
                name: if ty == EventType::PmuCustom { name } else { 0 },
                callstack: u64_list(row.get(9)?).into_iter().map(CallFrame::IP).collect(),
                user_regs: (regs_mask != 0).then(|| UserRegs {
                    abi: row.get(10).unwrap_or(0),
                    mask: regs_mask,
                    values: row.get(12).map(u64_list).unwrap_or_default(),
                }),
                user_stack: Vec::new(),
            });
        }
    }

    if session.has_table("events") {
        let mut statement = session.connection().prepare(
            "SELECT e.timestamp, e.event_id, e.instance, e.parent_id, e.flow_id, e.\"type\",
                    e.pid, e.tid, e.value, p.function_id, p.file_id, p.line
             FROM events e LEFT JOIN payloads p ON p.event_id = e.event_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let kind: u8 = row.get(5)?;
            let event_id: u64 = row.get(1)?;
            let ty = if kind == EventKind::Begin as u8 {
                EventType::RooflineLoopStart
            } else if kind == EventKind::End as u8 {
                EventType::RooflineLoopEnd
            } else {
                utils::event_type_from_name(
                    strings.get(&event_id).map(String::as_str).unwrap_or(""),
                )
                .unwrap_or(EventType::PmuCustom)
            };
            let location = row
                .get::<_, Option<u64>>(9)?
                .map(|function_name| Location {
                    function_name,
                    file_name: row.get(10).unwrap_or(0),
                    line: row.get(11).unwrap_or(0),
                });
            events.push(Event {
                unique_id: row.get(2)?,
                correlation_id: row.get(4)?,
                parent_id: row.get(3)?,
                ty,
                thread_id: row.get(7)?,
                process_id: row.get(6)?,
                cpu: u32::MAX,
                time_enabled: 0,
                time_running: 0,
                value: row.get::<_, i64>(8)? as u64,
                timestamp: row.get::<_, i64>(0)? as u64,
                name: 0,
                callstack: location.into_iter().map(CallFrame::Location).collect(),
                user_regs: None,
                user_stack: Vec::new(),
            });
        }
    }

    events.sort_by_key(|event| event.timestamp);
    Ok(serde_json::to_string_pretty(&events)?)
}

fn u64_list(value: Value) -> Vec<u64> {
    let Value::List(items) = value else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            Value::UBigInt(value) => Some(value),
            Value::BigInt(value) => Some(value as u64),
            _ => None,
        })
        .collect()
}
