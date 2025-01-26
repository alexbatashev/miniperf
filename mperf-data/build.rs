fn main() {
    ::capnpc::CompilerCommand::new()
        .file("schema/event.capnp")
        .run()
        .expect("compiling schema");
    ::capnpc::CompilerCommand::new()
        .file("schema/ipc_message.capnp")
        .run()
        .expect("compiling schema");
}
