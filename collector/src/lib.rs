use atomic_counter::{AtomicCounter, ConsistentCounter};
use lazy_static::lazy_static;
use std::sync::Mutex;

use mperf_data::Event;
use shmem::proc_channel::Sender;

pub mod ffi;

lazy_static! {
    static ref SENDER: Mutex<Sender<Event>> = {
        let name = std::env::var("MPERF_COLLECTOR_SHMEM_ID")
            .expect("MPERF_COLLECTOR_SHMEM_ID must be set by the caller");

        Mutex::new(Sender::create(&name, 8192).expect("failed to open shared memory"))
    };
    static ref ID_COUNTER: ConsistentCounter = {
        let start = std::env::var("MPERF_COLLECTOR_IDS_START")
            .expect("MPERF_COLLECTOR_SHMEM_ID must be set by the caller");
        ConsistentCounter::new(
            start
                .parse::<usize>()
                .expect("MPERF_COLLECTOR_IDS_START must be an unsigned integer"),
        )
    };
}

pub fn send_event(evt: Event) -> Result<(), Box<dyn std::error::Error>> {
    let sender = SENDER.lock()?;
    sender.send_sync(evt)?;

    Ok(())
}

pub fn get_next_id() -> u64 {
    ID_COUNTER.inc() as u64
}
