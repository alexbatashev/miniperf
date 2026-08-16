use std::ffi::{CString, c_void};
use std::path::PathBuf;

fn collector_cdylib() -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    dir.pop();
    let candidate = dir.join("libmperf_collector.so");
    candidate.exists().then_some(candidate)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IttId {
    d1: u64,
    d2: u64,
    d3: u64,
}

#[test]
fn itt_collector_records_tasks() {
    let candidate = {
        let mut dir = std::env::current_exe().unwrap();
        dir.pop();
        dir.pop();
        dir.join("libmperf_itt.so")
    };
    if !candidate.exists() {
        eprintln!("skipping: build libmperf_itt.so first (cargo build -p miniperf-shim-itt)");
        return;
    }
    let shim = candidate;
    let Some(collector) = collector_cdylib() else {
        eprintln!("skipping: build libmperf_collector.so first");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MPERF_SESSION_DIR", dir.path());
        std::env::set_var("MPERF_COLLECTOR_LIBRARY", &collector);
    }

    let path = CString::new(shim.to_str().unwrap()).unwrap();
    let lib = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
    assert!(!lib.is_null(), "failed to dlopen itt shim");
    let sym = |name: &str| {
        let name = CString::new(name).unwrap();
        let symbol = unsafe { libc::dlsym(lib, name.as_ptr()) };
        assert!(!symbol.is_null(), "missing symbol");
        symbol
    };
    unsafe {
        let domain_create: extern "C" fn(*const i8) -> *mut c_void =
            std::mem::transmute(sym("__itt_domain_create"));
        let string_create: extern "C" fn(*const i8) -> *mut c_void =
            std::mem::transmute(sym("__itt_string_handle_create"));
        let task_begin: extern "C" fn(*const c_void, IttId, IttId, *mut c_void) =
            std::mem::transmute(sym("__itt_task_begin"));
        let task_end: extern "C" fn(*const c_void) = std::mem::transmute(sym("__itt_task_end"));

        let domain_name = CString::new("tbb").unwrap();
        let task_name = CString::new("flow_node").unwrap();
        let domain = domain_create(domain_name.as_ptr());
        assert_eq!(domain, domain_create(domain_name.as_ptr()));
        let name = string_create(task_name.as_ptr());
        for _ in 0..100 {
            task_begin(domain, IttId::default(), IttId::default(), name);
            task_end(domain);
        }
        let shutdown: extern "C" fn() = std::mem::transmute(
            libc::dlsym(libc::RTLD_DEFAULT, CString::new("mperf_trace_shutdown").unwrap().as_ptr()),
        );
        shutdown();
    }

    let session = store::Session::open(dir.path()).unwrap();
    let begins: i64 = session
        .connection()
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM events e \
             JOIN payloads p ON p.event_id = e.event_id \
             JOIN strings s ON s.id = p.name_id \
             WHERE s.string = 'flow_node' AND e.type = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(begins, 100);
    let paired_ends: i64 = session
        .connection()
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM events e WHERE e.type = 1 AND e.flow_id IN \
             (SELECT instance FROM events WHERE type = 0)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(paired_ends, 100);
}
