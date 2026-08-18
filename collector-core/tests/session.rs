use std::ffi::CString;

use mperf_collector::{MperfTracePayload, TraceKind};

#[test]
fn records_spans_counters_and_stacks() {
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MPERF_SESSION_DIR", dir.path()) };

    let name = CString::new("phase").unwrap();
    let function = CString::new("main").unwrap();
    let file = CString::new("app.c").unwrap();
    let payload = MperfTracePayload {
        name: name.as_ptr(),
        function: function.as_ptr(),
        file: file.as_ptr(),
        line: 42,
        column: 3,
        flags: mperf_collector::MPERF_TRACE_FLAG_STACK,
    };
    let handle = unsafe { mperf_collector::mperf_trace_register(&payload) };
    assert!(!handle.is_null());
    let same = unsafe { mperf_collector::mperf_trace_register(&payload) };
    assert_eq!(
        unsafe { (*handle).event_id },
        unsafe { (*same).event_id },
        "same payload must mint the same event id"
    );

    let workers: Vec<_> = (0..4)
        .map(|_| {
            let handle = handle as usize;
            std::thread::spawn(move || {
                let handle = handle as *mut mperf_collector::HandleData;
                for value in 0..1000 {
                    let instance = unsafe { mperf_collector::mperf_trace_begin(handle, 0) };
                    assert_ne!(instance, 0);
                    unsafe { mperf_collector::mperf_trace_counter(handle, value) };
                    unsafe { mperf_collector::mperf_trace_end(handle, instance) };
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    mperf_collector::mperf_trace_shutdown();

    let session = store::Session::open(dir.path()).unwrap();
    for table in ["events", "strings", "payloads", "clock"] {
        assert!(session.has_table(table), "missing table {table}");
    }
    let conn = session.connection();
    let begins: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*)::BIGINT FROM events WHERE type = {}",
                TraceKind::Begin as u8
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(begins, 4000);
    let counters: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*)::BIGINT FROM events WHERE type = {}",
                TraceKind::Counter as u8
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(counters, 4000);
    let named: i64 = conn
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM events e \
             JOIN payloads p ON p.event_id = e.event_id \
             JOIN strings s ON s.id = p.name_id WHERE s.string = 'phase'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(named, 12000);
    let anchors: i64 = conn
        .query_row("SELECT COUNT(*)::BIGINT FROM clock", [], |row| row.get(0))
        .unwrap();
    assert_eq!(anchors, 2);
    if session.has_table("stacks") {
        let stacks: i64 = conn
            .query_row("SELECT COUNT(*)::BIGINT FROM stacks", [], |row| row.get(0))
            .unwrap();
        assert!(stacks >= 1);
    }
}
