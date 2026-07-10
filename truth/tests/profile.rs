use std::{
    env, fs,
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use sqlite::State;
use truth::assert_f6_1_duty_split;

const DUTY_SPLIT_FP: &str = env!("TRUTH_DUTY_SPLIT_FP");
const DUTY_SPLIT_NO_FP: &str = env!("TRUTH_DUTY_SPLIT_NO_FP");

#[test]
#[ignore = "requires Linux perf_event access; run `cargo build -p mperf && cargo test -p truth --test profile -- --ignored`"]
fn f6_1_mperf_reports_analytic_duty_split() {
    let Some((results, log_path)) = record_fixture(DUTY_SPLIT_FP, "3", "01-F6.1") else {
        return;
    };
    let counts = instruction_sample_counts(&results.join("perf.db"), "01-F6.1");
    cleanup_recording(&results, &log_path, "01-F6.1");
    assert_f6_1_duty_split(counts[0], counts[1]);
}

#[test]
#[ignore = "requires Linux perf_event access; run `cargo build -p mperf && cargo test -p truth --test profile -- --ignored`"]
fn f3_1_dwarf_resolves_optimized_no_frame_pointer_fixture() {
    let Some((results, log_path)) = record_fixture(DUTY_SPLIT_NO_FP, "1", "01-F3.1") else {
        return;
    };
    let counts = hotspot_instruction_counts(&results.join("perf.db"), "01-F3.1");
    assert!(
        counts.into_iter().all(|count| count > 0),
        "01-F3.1 DWARF: optimized no-frame-pointer fixture did not resolve both duty functions: {counts:?}"
    );

    let folded = fs::read_to_string(results.join("flamegraph_instructions.folded"))
        .expect("01-F3.1 DWARF: missing instruction flamegraph");
    let has_multiframe_duty_stack = folded.lines().any(|line| {
        let stack = line.rsplit_once(' ').map_or(line, |(stack, _)| stack);
        stack.contains(';') && (stack.contains("duty_60") || stack.contains("duty_40"))
    });
    assert!(
        has_multiframe_duty_stack,
        "01-F3.1 DWARF: no multi-frame duty stack was emitted for the optimized no-frame-pointer fixture"
    );
    cleanup_recording(&results, &log_path, "01-F3.1");
}

fn record_fixture(fixture: &str, duration: &str, milestone: &str) -> Option<(PathBuf, PathBuf)> {
    if !perf_events_are_available() {
        eprintln!(
            "{milestone} skipped: perf_event access is unavailable (set kernel.perf_event_paranoid=-1 or grant equivalent capabilities)"
        );
        return None;
    }

    let mperf = mperf_binary();
    assert!(
        mperf.is_file(),
        "{milestone}: mperf binary not found at {}; run `cargo build -p mperf` first",
        mperf.display()
    );
    let results = unique_results_dir();
    let log_path = results.with_extension("log");
    let log = File::create(&log_path).expect("01-F6.1: failed to create profiler log");
    let status = Command::new("timeout")
        .args(["45s"])
        .arg(&mperf)
        .args(["record", "--scenario", "snapshot", "--output-directory"])
        .arg(&results)
        .args(["--", fixture, duration])
        .stdout(Stdio::from(
            log.try_clone()
                .expect("01-F6.1: failed to clone profiler log"),
        ))
        .stderr(Stdio::from(log))
        .status()
        .expect("01-F6.1: failed to launch mperf");
    let log = fs::read_to_string(&log_path).expect("01-F6.1: failed to read profiler log");
    assert!(
        status.success(),
        "{milestone}: mperf failed or exceeded 45 seconds\n{log}"
    );
    Some((results, log_path))
}

fn cleanup_recording(results: &Path, log_path: &Path, milestone: &str) {
    fs::remove_dir_all(results)
        .unwrap_or_else(|error| panic!("{milestone}: failed to remove results: {error}"));
    fs::remove_file(log_path)
        .unwrap_or_else(|error| panic!("{milestone}: failed to remove profiler log: {error}"));
}

fn instruction_sample_counts(database: &Path, milestone: &str) -> [u64; 2] {
    query_duty_counts(
        database,
        milestone,
        "SELECT proc_map.func_name, COUNT(*) AS value \
         FROM pmu_counters INNER JOIN proc_map ON pmu_counters.ip = proc_map.ip \
         WHERE pmu_counters.pmu_instructions > 0 \
           AND (proc_map.func_name LIKE 'duty_60%' OR proc_map.func_name LIKE 'duty_40%') \
         GROUP BY proc_map.func_name",
        "value",
    )
}

fn hotspot_instruction_counts(database: &Path, milestone: &str) -> [u64; 2] {
    query_duty_counts(
        database,
        milestone,
        "SELECT func_name, instructions AS value FROM hotspots \
         WHERE func_name LIKE 'duty_60%' OR func_name LIKE 'duty_40%'",
        "value",
    )
}

fn query_duty_counts(
    database: &Path,
    milestone: &str,
    query: &str,
    value_column: &str,
) -> [u64; 2] {
    let connection = sqlite::open(database).unwrap_or_else(|error| {
        panic!(
            "{milestone}: failed to open profiler database {}: {error}",
            database.display()
        )
    });
    let mut statement = connection
        .prepare(query)
        .unwrap_or_else(|error| panic!("{milestone}: failed to query hotspots: {error}"));
    let mut counts = [0_u64; 2];
    while let Ok(State::Row) = statement.next() {
        let name = statement
            .read::<String, _>("func_name")
            .expect("01-F6.1: invalid function name");
        let value = statement
            .read::<Option<i64>, _>(value_column)
            .expect("01-F6.1: invalid instruction count");
        let slot = if name.starts_with("duty_60") {
            0
        } else if name.starts_with("duty_40") {
            1
        } else {
            continue;
        };
        counts[slot] += value.unwrap_or(0).max(0) as u64;
    }
    counts
}

fn perf_events_are_available() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(value) = fs::read_to_string("/proc/sys/kernel/perf_event_paranoid") else {
        return false;
    };
    value.trim().parse::<i32>().is_ok_and(|level| level <= -1)
}

fn mperf_binary() -> PathBuf {
    if let Some(path) = env::var_os("MPERF_BIN") {
        return path.into();
    }
    let mut path = env::current_exe().expect("01-F6.1: cannot locate test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "mperf.exe" } else { "mperf" });
    path
}

fn unique_results_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("01-F6.1: system clock predates Unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("mperf-truth-{}-{nonce}", std::process::id()))
}
