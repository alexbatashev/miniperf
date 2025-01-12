fn main() {
    ::capnpc::CompilerCommand::new()
        .file("schema/event.capnp")
        .run()
        .expect("compiling schema");
}
