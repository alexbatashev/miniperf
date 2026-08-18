fn main() {
    println!("cargo:rerun-if-changed=src/roofline/calibrate_rvv.c");

    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv64") {
        cc::Build::new()
            .file("src/roofline/calibrate_rvv.c")
            .opt_level(3)
            .flag("-march=rv64gcv")
            .flag("-mabi=lp64d")
            .compile("mperf_roofline_rvv");
    }
}
