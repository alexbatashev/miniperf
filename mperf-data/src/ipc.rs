use std::io::Cursor;

use crate::{ipc_message_capnp, Event};

#[derive(Clone, Debug)]
pub struct IPCString {
    pub key: u64,
    pub value: String,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum IPCMessage {
    String(IPCString),
    Event(Event),
}

impl shmem::proc_channel::Sendable for IPCMessage {
    fn as_raw_bytes(&self) -> Vec<u8> {
        let mut builder = capnp::message::Builder::new_default();
        let mut ipc_message = builder.init_root::<ipc_message_capnp::ipc_message::Builder>();
        ipc_message.set_message(self);

        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);

        capnp::serialize_packed::write_message(&mut cursor, &builder).expect("failed to serialize");

        let len = buffer.len();

        let mut res = Vec::with_capacity(std::mem::size_of::<usize>() + len);

        res.extend(len.to_le_bytes());
        res.extend(buffer);

        res
    }

    fn from_raw_bytes(bytes: &[u8]) -> Self {
        const OFFSET: usize = std::mem::size_of::<usize>();

        let mut len_bytes = [0u8; OFFSET];
        len_bytes.copy_from_slice(&bytes[0..OFFSET]);
        let len = usize::from_le_bytes(len_bytes);
        let reader = capnp::serialize_packed::read_message(
            &bytes[OFFSET..(OFFSET + len)],
            capnp::message::ReaderOptions::new(),
        )
        .expect("failed to read message");
        let ipc_message = reader
            .get_root::<ipc_message_capnp::ipc_message::Reader>()
            .unwrap();

        ipc_message.into()
    }
}
