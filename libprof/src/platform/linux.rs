//! Linux implementations of the host facilities in [`super`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

use super::{ProcessContext, ProcessIo, ProcessStat};
use crate::sink::ProcAddr;

pub(super) fn process_tree(root_pid: u32) -> Option<Vec<ProcessStat>> {
    let mut all = HashMap::<u32, ProcessStat>::new();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
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
        return Some(Vec::new());
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
    Some(result)
}

fn read_proc_stat(pid: u32) -> Option<ProcessStat> {
    let value = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    let command = value[open + 1..close].to_string();
    let fields = value[close + 2..].split_whitespace().collect::<Vec<_>>();
    Some(ProcessStat {
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

pub(super) fn process_io(pid: u32) -> ProcessIo {
    let values = key_values(&format!("/proc/{pid}/io"));
    ProcessIo {
        read_bytes: get_u64(&values, "read_bytes"),
        write_bytes: get_u64(&values, "write_bytes"),
        read_calls: get_u64(&values, "syscr"),
        write_calls: get_u64(&values, "syscw"),
    }
}

pub(super) fn process_context(pid: u32) -> ProcessContext {
    let values = key_values(&format!("/proc/{pid}/status"));
    ProcessContext {
        voluntary: get_u64(&values, "voluntary_ctxt_switches"),
        involuntary: get_u64(&values, "nonvoluntary_ctxt_switches"),
    }
}

/// `key: value` lines, as `/proc/<pid>/status` and `/proc/<pid>/io` write them.
pub(crate) fn key_values(path: &str) -> HashMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_string(), value.trim().to_string()))
        .collect()
}

/// The first integer of a `key_values` entry, zero when absent or unparsable.
pub(crate) fn get_u64(values: &HashMap<String, String>, key: &str) -> u64 {
    values
        .get(key)
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

pub(super) fn process_modules(pid: u32) -> Vec<ProcAddr> {
    let Ok(maps) = proc_maps::get_process_maps(pid as proc_maps::Pid) else {
        return Vec::new();
    };
    maps.into_iter()
        .filter(|map| map.is_exec())
        .filter_map(|map| {
            Some(ProcAddr {
                pid,
                addr: map.start() as u64,
                len: map.size() as u64,
                // Linux executable mappings normally begin at a non-zero ELF
                // file offset. Dropping it creates a second, overlapping module
                // with a bogus load bias alongside PERF_RECORD_MMAP, and which
                // one the unwinder sees first then depends on hash iteration
                // order — two identical recordings unwind into unrelated
                // functions.
                pgoff: map.offset as u64,
                filename: map.filename()?.to_string_lossy().into_owned(),
            })
        })
        .collect()
}

pub(super) fn current_thread_id() -> u64 {
    unsafe { libc::gettid() as u64 }
}
