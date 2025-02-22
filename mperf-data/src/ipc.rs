use std::{
    cell::Cell,
    thread::{self, JoinHandle},
};

use capnp::capability::Promise;

use crate::{ipc_message_capnp, Event};

#[derive(Clone, Debug)]
pub struct IPCString {
    pub key: u64,
    pub value: String,
}

#[derive(Clone, Debug)]
pub enum IPCMessage {
    String(IPCString),
    Event(Event),
}

impl shmem::proc_channel::Sendable for IPCMessage {
    fn as_raw_bytes(&self) -> Vec<u8> {
        todo!()
    }

    fn from_raw_bytes(bytes: &[u8]) -> Self {
        todo!()
    }
}
