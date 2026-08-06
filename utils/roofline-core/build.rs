mod tmdl_spec;

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=spec/riscv-ast-v1.json");
    println!("cargo:rerun-if-changed=tmdl_spec.rs");

    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("riscv_operations.rs");
    tmdl_spec::generate("spec/riscv-ast-v1.json", &output)
        .expect("generate RISC-V Roofline operation table from TMDL");
}
