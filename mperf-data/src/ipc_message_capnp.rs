use crate::IPCMessage;

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
