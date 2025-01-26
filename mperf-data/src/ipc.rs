use capnp::capability::Promise;
use capnp_rpc::pry;

use crate::{ipc_message_capnp, Event};

pub use ipc_message_capnp::ipc_service::Client as IPCClient;

pub struct IPCString {
    pub key: u64,
    pub value: String,
}

pub enum IPCMessage {
    String(IPCString),
    Event(Event),
}

pub struct IPCServer {
    callback: Box<dyn Fn(IPCMessage)>,
}

impl IPCServer {
    pub fn new(callback: Box<dyn Fn(IPCMessage)>) -> Self {
        Self { callback }
    }
}

impl ipc_message_capnp::ipc_service::Server for IPCServer {
    fn post(
        &mut self,
        params: ipc_message_capnp::ipc_service::PostParams,
        _results: ipc_message_capnp::ipc_service::PostResults,
    ) -> Promise<(), capnp::Error> {
        let message = pry!(pry!(params.get()).get_message());

        let message = match message.which() {
            Ok(ipc_message_capnp::ipc_message::String(Ok(string))) => {
                IPCMessage::String(IPCString {
                    key: string.get_key(),
                    value: string
                        .get_value()
                        .expect("failed to get string value")
                        .to_string()
                        .expect("failed to get string value"),
                })
            }
            Ok(ipc_message_capnp::ipc_message::Event(Ok(event))) => {
                IPCMessage::Event(Event::from(event))
            }
            _ => unimplemented!(),
        };

        (self.callback)(message);

        Promise::ok(())
    }
}
