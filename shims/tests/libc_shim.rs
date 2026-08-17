use std::path::PathBuf;
use std::process::Command;

fn collector_cdylib() -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    dir.pop();
    let candidate = dir.join("libmperf_collector.so");
    candidate.exists().then_some(candidate)
}

fn shim_cdylib(name: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    dir.pop();
    let candidate = dir.join(name);
    candidate.exists().then_some(candidate)
}

#[test]
fn preload_records_allocations() {
    let Some(shim) = shim_cdylib("libmperf_libc.so") else {
        eprintln!("skipping: build libmperf_libc.so first (cargo build -p miniperf-shim-libc)");
        return;
    };
    let Some(collector) = collector_cdylib() else {
        eprintln!(
            "skipping: build libmperf_collector.so first (cargo build -p miniperf-collector-core)"
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .env("LD_PRELOAD", shim)
        .env("MPERF_SESSION_DIR", dir.path())
        .env("MPERF_COLLECTOR_LIBRARY", &collector)
        .env("MPERF_LIBC_SAMPLE_EVERY", "1")
        .status()
        .unwrap();
    assert!(status.success());

    let session = store::Session::open(dir.path()).unwrap();
    assert!(session.has_table("events"), "no events table written");
    let mallocs: i64 = session
        .connection()
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM events e \
             JOIN payloads p ON p.event_id = e.event_id \
             JOIN strings s ON s.id = p.name_id WHERE s.string = 'malloc'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        mallocs > 0,
        "expected malloc events from the preloaded shell"
    );
    let rates: i64 = session
        .connection()
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM events e \
             JOIN payloads p ON p.event_id = e.event_id \
             JOIN strings s ON s.id = p.name_id \
             WHERE s.string IN ('libc_sample_every', 'libc_size_threshold')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rates, 2, "effective throttle rates must be recorded");
}
