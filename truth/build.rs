use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=fixtures/duty_split.c");
    println!("cargo:rerun-if-changed=fixtures/known_sleeper.c");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo must set target OS");

    for (source, name) in [
        ("fixtures/duty_split.c", "duty_split"),
        ("fixtures/known_sleeper.c", "known_sleeper"),
    ] {
        for (suffix, frame_pointer_flag) in [
            ("fp", "-fno-omit-frame-pointer"),
            ("no-fp", "-fomit-frame-pointer"),
        ] {
            let output = out_dir.join(format!("{name}-{suffix}"));
            let mut command = Command::new(&compiler);
            command.args([
                "-O2",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                frame_pointer_flag,
                "-fno-inline",
            ]);
            if target_os == "linux" {
                // Keep fixture IPs unambiguous while the suite validates the
                // profiler's symbol and stack attribution rather than the
                // platform's PIE relocation policy.
                command.args(["-fno-pie", "-no-pie"]);
            }
            let status = command
                .args([source, "-o"])
                .arg(&output)
                .status()
                .unwrap_or_else(|error| panic!("failed to run {:?}: {error}", compiler));
            assert!(status.success(), "failed to build fixture {source}");

            let env_name = format!(
                "TRUTH_{}_{}",
                name.to_ascii_uppercase(),
                suffix.replace('-', "_").to_ascii_uppercase()
            );
            println!("cargo:rustc-env={env_name}={}", output.display());
        }
    }
}
