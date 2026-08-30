use std::{
    env, fs,
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use store::{duckdb::Row, Session};
use truth::assert_f6_1_duty_split;

const DUTY_SPLIT_FP: &str = env!("TRUTH_DUTY_SPLIT_FP");
const DUTY_SPLIT_NO_FP: &str = env!("TRUTH_DUTY_SPLIT_NO_FP");
const POINTER_CHASE_FP: &str = env!("TRUTH_POINTER_CHASE_FP");
const BRANCH_HEAVY_FP: &str = env!("TRUTH_BRANCH_HEAVY_FP");

#[test]
fn f6_1_mperf_reports_analytic_duty_split() {
    let Some((results, log_path)) = record_fixture(DUTY_SPLIT_FP, "3", "01-F6.1") else {
        return;
    };
    let counts = instruction_sample_counts(&results, "01-F6.1");
    cleanup_recording(&results, &log_path, "01-F6.1");
    assert_f6_1_duty_split(counts[0], counts[1]);
}

#[test]
fn f3_1_dwarf_resolves_optimized_no_frame_pointer_fixture() {
    let Some((results, log_path)) = record_fixture(DUTY_SPLIT_NO_FP, "1", "01-F3.1") else {
        return;
    };
    let counts = hotspot_instruction_counts(&results, "01-F3.1");
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

#[test]
fn tma_acceptance_fixtures_have_expected_dominant_paths() {
    for (fixture, expected) in [
        (POINTER_CHASE_FP, "be_bound"),
        (BRANCH_HEAVY_FP, "bad_speculation"),
    ] {
        let Some((results, log)) = record_tma_fixture(fixture, "13-TMA") else {
            return;
        };
        let session = Session::open(&results).expect("13-TMA: open session");
        let mut statement = session
            .connection()
            .prepare("SELECT metric FROM tma_summary WHERE verdict = 'dominant'")
            .expect("13-TMA: query verdict");
        let mut rows = statement.query([]).expect("13-TMA: run verdict query");
        let dominant = rows
            .next()
            .expect("13-TMA: read verdict")
            .expect("13-TMA: missing dominant verdict")
            .get::<_, String>("metric")
            .expect("13-TMA: read metric");
        assert!(
            dominant == expected || (expected == "be_bound" && dominant.starts_with("be_bound.")),
            "13-TMA: expected {expected}, got {dominant}"
        );
        assert_fixed_topdown_fractions_are_exhaustive(&session);
        cleanup_recording(&results, &log, "13-TMA");
    }
}

/// On a host that resolved to a hardware topdown rung the level-one metrics are
/// counted, not estimated, so they must account for every issue slot. Hosts at
/// the arithmetic baseline have no such guarantee and are skipped.
fn assert_fixed_topdown_fractions_are_exhaustive(session: &Session) {
    let rung: String = session
        .connection()
        .query_row(
            "SELECT rung FROM capture_fidelity WHERE scenario = 'tma' AND status = 'chosen'",
            [],
            |row| row.get(0),
        )
        .expect("13-TMA: read capture fidelity");
    if !matches!(rung.as_str(), "fixed_topdown" | "arm_slots_topdown") {
        eprintln!("13-TMA: fixed topdown check skipped: host recorded at '{rung}'");
        return;
    }
    // A heterogeneous host reports one `<metric>.<core type>` set per core
    // type; each set covers that core type's slots on its own.
    let connection = session.connection();
    let mut statement = connection
        .prepare(
            "SELECT SPLIT_PART(metric, '.', 2) AS core, SUM(value) AS total
             FROM tma_summary GROUP BY core HAVING SUM(value) IS NOT NULL",
        )
        .expect("13-TMA: query level-one totals");
    let totals = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>("core")?, row.get::<_, f64>("total")?))
        })
        .expect("13-TMA: run level-one totals")
        .collect::<Result<Vec<_>, _>>()
        .expect("13-TMA: read level-one totals");
    assert!(!totals.is_empty(), "13-TMA: {rung} produced no metrics");
    for (core, total) in totals {
        assert!(
            (total - 1.0).abs() < 0.02,
            "13-TMA: {rung} level-one fractions for '{core}' sum to {total}, not 1.0"
        );
    }
}

fn record_fixture(fixture: &str, duration: &str, milestone: &str) -> Option<(PathBuf, PathBuf)> {
    if !perf_events_are_available() {
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

fn record_tma_fixture(fixture: &str, milestone: &str) -> Option<(PathBuf, PathBuf)> {
    if !perf_events_are_available() {
        return None;
    }
    let mperf = mperf_binary();
    let results = unique_results_dir();
    let log_path = results.with_extension("log");
    let log = File::create(&log_path).expect("13-TMA: create log");
    let status = Command::new("timeout")
        .args(["60s"])
        .arg(&mperf)
        .args(["record", "--scenario", "tma", "--output-directory"])
        .arg(&results)
        .args(["--", fixture])
        .stdout(Stdio::from(log.try_clone().expect("13-TMA: clone log")))
        .stderr(Stdio::from(log))
        .status()
        .expect("13-TMA: run mperf");
    assert!(status.success(), "{milestone}: TMA record failed");
    Some((results, log_path))
}

fn cleanup_recording(results: &Path, log_path: &Path, milestone: &str) {
    fs::remove_dir_all(results)
        .unwrap_or_else(|error| panic!("{milestone}: failed to remove results: {error}"));
    fs::remove_file(log_path)
        .unwrap_or_else(|error| panic!("{milestone}: failed to remove profiler log: {error}"));
}

fn instruction_sample_counts(results: &Path, milestone: &str) -> [u64; 2] {
    query_duty_counts(
        results,
        milestone,
        "SELECT proc_map.func_name AS func_name, COUNT(*) AS value \
         FROM pmu_counters INNER JOIN proc_map ON pmu_counters.ip = proc_map.ip \
         WHERE pmu_counters.pmu_instructions > 0 \
           AND (proc_map.func_name LIKE 'duty_60%' OR proc_map.func_name LIKE 'duty_40%') \
         GROUP BY proc_map.func_name",
    )
}

fn hotspot_instruction_counts(results: &Path, milestone: &str) -> [u64; 2] {
    query_duty_counts(
        results,
        milestone,
        "SELECT func_name, instructions AS value FROM hotspots \
         WHERE func_name LIKE 'duty_60%' OR func_name LIKE 'duty_40%'",
    )
}

fn query_duty_counts(results: &Path, milestone: &str, query: &str) -> [u64; 2] {
    let session = Session::open(results).unwrap_or_else(|error| {
        panic!(
            "{milestone}: failed to open profiler session {}: {error}",
            results.display()
        )
    });
    let mut statement = session
        .connection()
        .prepare(query)
        .unwrap_or_else(|error| panic!("{milestone}: failed to query hotspots: {error}"));
    let mut rows = statement
        .query([])
        .unwrap_or_else(|error| panic!("{milestone}: failed to run query: {error}"));
    let mut counts = [0_u64; 2];
    while let Ok(Some(row)) = rows.next() {
        let (name, value) = duty_row(row);
        let slot = if name.starts_with("duty_60") {
            0
        } else if name.starts_with("duty_40") {
            1
        } else {
            continue;
        };
        counts[slot] += value;
    }
    counts
}

fn duty_row(row: &Row<'_>) -> (String, u64) {
    let name = row
        .get::<_, String>("func_name")
        .expect("01-F6.1: invalid function name");
    let value = row
        .get::<_, Option<i64>>("value")
        .expect("01-F6.1: invalid instruction count");
    (name, value.unwrap_or(0).max(0) as u64)
}

/// Whether the profiler can be run here. Hosts without perf access must opt
/// out with `MPERF_NO_PMU=1`; a missing PMU is never a silent pass.
fn perf_events_are_available() -> bool {
    if env::var_os("MPERF_NO_PMU").is_some() {
        eprintln!("skipped: MPERF_NO_PMU is set");
        return false;
    }
    let level = fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok());
    assert!(
        level.is_some_and(|level| level <= -1),
        "perf_event access is unavailable (kernel.perf_event_paranoid={level:?}); \
         set kernel.perf_event_paranoid=-1, or set MPERF_NO_PMU=1 to skip the profiler tests explicitly"
    );
    true
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
    if !path.is_file() {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "mperf"])
            .status()
            .expect("failed to run cargo build -p mperf");
        assert!(status.success(), "cargo build -p mperf failed");
    }
    path
}

fn unique_results_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("01-F6.1: system clock predates Unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("mperf-truth-{}-{nonce}", std::process::id()))
}
