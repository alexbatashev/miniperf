use clap::ValueEnum;
use serde::{Deserialize, Serialize};

mod event;
pub(crate) mod event_capnp;
mod ipc;
mod ipc_message_capnp;

pub use event::{CallFrame, Event, EventType, IString, Location, ProcMap, ProcMapEntry};
pub use ipc::{IPCClient, IPCMessage, IPCServer, IPCString};

#[derive(Clone, Debug, Copy, ValueEnum, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scenario {
    Snapshot,
    Roofline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordInfo {
    pub scenario: Scenario,
    pub command: Option<Vec<String>>,
    pub pid: Option<u32>,
}

impl Scenario {
    pub fn name(&self) -> &'static str {
        match self {
            Scenario::Snapshot => "Snapshot",
            Scenario::Roofline => "Roofline",
        }
    }
}
