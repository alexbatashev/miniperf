use lazy_static::lazy_static;
use std::{cell::RefCell, sync::Mutex};

use mperf_data::Event;
use shmem::proc_channel::Sender;

pub mod ffi;

lazy_static! {
    static ref SENDER: Mutex<Sender<Event>> = {
        let name = std::env::var("MPERF_COLLECTOR_SHMEM_ID")
            .expect("MPERF_COLLECTOR_SHMEM_ID must be set by the caller");

        Mutex::new(Sender::attach(&name, 8192).expect("failed to open shared memory"))
    };
    static ref PROFILING_ENABLED: bool = std::env::var("MPERF_COLLECTOR_ENABLED").is_ok();
    static ref ROOFLINE_INSTR_ENABLED: bool =
        std::env::var("MPERF_COLLECTOR_ROOFLINE_INSTRUMENTED").is_ok();
}

thread_local! {
    static LAST_ID: RefCell<u64> = const { RefCell::new(0) };
}

pub fn send_event(evt: Event) -> Result<(), Box<dyn std::error::Error>> {
    let sender = SENDER.lock()?;
    sender.send_sync(evt)?;

    Ok(())
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
