use clap::ValueEnum;
use serde::{Deserialize, Serialize};

mod event;

pub use event::{Event, EventType, IString};

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
