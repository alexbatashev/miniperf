use lazy_static::lazy_static;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use shmem::proc_channel::Sender;
use std::{cell::RefCell, collections::HashMap, sync::Mutex};

use mperf_data::{Event, IPCMessage, IPCString};

pub mod ffi;
const SIZE_16MB: usize = 16 * 1024 * 1024;

lazy_static! {
    static ref SENDER: Mutex<Sender<IPCMessage>> = {
        let name = std::env::var("MPERF_COLLECTOR_SHMEM_ID")
            .expect("MPERF_COLLECTOR_SHMEM_ID must be set by the caller");

        let mutex =
            Mutex::new(Sender::attach(&name, SIZE_16MB).expect("failed to open shared memory"));

        unsafe {
            libc::atexit(close_pipe);
        }
        mutex
    };
    static ref STRINGS: RwLock<HashMap<String, u128>> = RwLock::new(HashMap::new());
    static ref PROFILING_ENABLED: bool = std::env::var("MPERF_COLLECTOR_ENABLED").is_ok();
    static ref ROOFLINE_INSTR_ENABLED: bool =
        std::env::var("MPERF_COLLECTOR_ROOFLINE_INSTRUMENTED").is_ok();
}

thread_local! {
    static LAST_ID: RefCell<u64> = const { RefCell::new(0) };
}

pub fn send_event(evt: Event) -> Result<(), Box<dyn std::error::Error>> {
    let sender = SENDER.lock()?;
    let res = sender.send_sync(IPCMessage::Event(evt));

    if res.is_err() {
        eprintln!("Lost an event IPC message due to an error {:?}", res.err());
    }

    Ok(())
}

pub fn get_string_id(string: &str) -> u128 {
    let reader = STRINGS.upgradable_read();
    if reader.contains_key(string) {
        return *reader.get(string).unwrap();
    }

    let key = {
        let mut writer = RwLockUpgradableReadGuard::upgrade(reader);

        // We now have exclusive lock, double check no one has added our string
        if writer.contains_key(string) {
            return *writer.get(string).unwrap();
        }

        let id = uuid::Uuid::now_v7().as_u128();

        writer.insert(string.to_string(), id);

        id
    };

    let sender = SENDER.lock().unwrap();
    let res = sender.send_sync(IPCMessage::String(IPCString {
        key,
        value: string.to_string(),
    }));

    if res.is_err() {
        eprintln!("Lost a string IPC message due to an error {:?}", res.err());
    }

    key
}

pub fn get_next_id() -> u128 {
    let counter = LAST_ID.with_borrow_mut(|cnt| {
        let last = *cnt;
        *cnt += 1;
        last as u128
    });

    ((std::process::id() as u128) << 96) | ((unsafe { libc::gettid() as u128 }) << 64) | counter
}

pub fn profiling_enabled() -> bool {
    *PROFILING_ENABLED
}

pub fn roofline_instrumentation_enabled() -> bool {
    *ROOFLINE_INSTR_ENABLED
}

extern "C" fn close_pipe() {
    let sender = SENDER.lock().unwrap();
    let _ = sender.close();
}

pub(crate) fn get_timestamp() -> u64 {
    let mut ts: libc::timespec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut ts) };
    (ts.tv_sec * 1_000_000_000 + ts.tv_nsec) as u64
}
