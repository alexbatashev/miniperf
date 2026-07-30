fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // QEMU plugins resolve the QEMU and GLib APIs from the loading process.
        // Unlike ELF linkers, the macOS linker rejects those unresolved symbols
        // unless the bundle explicitly opts into dynamic lookup.
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
