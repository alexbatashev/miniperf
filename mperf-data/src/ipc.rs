use bincode::{Decode, Encode};

use crate::Event;

#[derive(Encode, Decode, Clone, Debug)]
pub struct IPCString {
    pub key: u64,
    pub value: String,
}

#[allow(clippy::large_enum_variant)]
#[derive(Encode, Decode, Clone, Debug)]
pub enum IPCMessage {
    String(IPCString),
    Event(Event),
}

impl shmem::proc_channel::Sendable for IPCMessage {
    fn as_raw_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).expect("Failed to encode message")
    }

    fn from_raw_bytes(bytes: &[u8]) -> Self {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .expect("Failed to decode message")
            .0
    }
}
