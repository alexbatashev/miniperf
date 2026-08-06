fn main() {
    println!("cargo:rerun-if-changed=src/roofline/calibrate_rvv.c");
    println!("cargo:rerun-if-changed=src/memory_preload.c");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let output = out_dir.join("libmperf_memory_preload.so");
        let status = std::process::Command::new("cc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&output)
            .arg("src/memory_preload.c")
            .args(["-ldl", "-pthread"])
            .status()
            .expect("compile memory allocation preload collector");
        assert!(
            status.success(),
            "compile memory allocation preload collector"
        );
        println!("cargo:rustc-env=MPERF_MEMORY_PRELOAD={}", output.display());
    }

    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv64") {
        cc::Build::new()
            .file("src/roofline/calibrate_rvv.c")
            .opt_level(3)
            .flag("-march=rv64gcv")
            .flag("-mabi=lp64d")
            .compile("mperf_roofline_rvv");
    }
}
