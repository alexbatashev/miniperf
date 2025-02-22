use crate::{ipc::IPCString, IPCMessage};

include!(concat!(env!("OUT_DIR"), "/schema/ipc_message_capnp.rs"));

impl ipc_message::Builder<'_> {
    pub fn set_message(&mut self, message: &IPCMessage) {
        match message {
            IPCMessage::String(string) => {
                let mut root = self.reborrow().init_string();
                root.set_key(string.key);
                root.set_value(string.value.clone());
            }
            IPCMessage::Event(event) => {
                let mut root = self.reborrow().init_event();
                root.set_event(event);
            }
        }
    }
}

impl From<ipc_message::Reader<'_>> for IPCMessage {
    fn from(value: ipc_message::Reader<'_>) -> Self {
        match value.reborrow().which() {
            Ok(ipc_message::Which::String(string)) => {
                let string = string.unwrap();
                let string = IPCString {
                    key: string.get_key(),
                    value: string.get_value().unwrap().to_string().unwrap(),
                };

                IPCMessage::String(string)
            }
            Ok(ipc_message::Which::Event(event)) => {
                let event = event.unwrap();
                IPCMessage::Event(event.into())
            }
            Err(_) => panic!(),
        }
    }
}
