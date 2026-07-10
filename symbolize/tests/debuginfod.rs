#![cfg(target_os = "linux")]

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use object::{Object, ObjectSegment, ObjectSymbol};
use symbolize::{BuildIdCache, ProcessMap, Resolver};

static ENVIRONMENT: OnceLock<Mutex<()>> = OnceLock::new();

struct SavedEnvironment(Vec<(&'static str, Option<OsString>)>);

impl SavedEnvironment {
    fn capture(names: &[&'static str]) -> Self {
        Self(
            names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
        )
    }
}

impl Drop for SavedEnvironment {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

fn run(command: &mut Command) {
    let status = command.status().expect("failed to start fixture tool");
    assert!(status.success(), "fixture tool failed: {command:?}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn resolve_fixture(
    binary: &Path,
    cache: &Path,
    mapped_start: u64,
    address: u64,
) -> Vec<symbolize::Frame> {
    Resolver::with_cache(
        [ProcessMap {
            pid: 1,
            path: binary.to_path_buf(),
            start: mapped_start,
            end: u64::MAX,
            offset: 0,
        }],
        BuildIdCache::new(cache),
    )
    .resolve(1, address)
}

#[test]
fn downloads_split_debug_info_once_then_resolves_from_cache() {
    let _environment = ENVIRONMENT
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _saved_environment = SavedEnvironment::capture(&[
        "PATH",
        "DEBUGINFOD_URLS",
        "MINIPERF_DEBUGINFOD",
        "FAKE_DEBUGINFOD_FILE",
        "FAKE_DEBUGINFOD_LOG",
    ]);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is before Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "miniperf-debuginfod-{}-{nonce}",
        std::process::id()
    ));
    let bin_dir = root.join("bin");
    let source = root.join("fixture.c");
    let binary = root.join("fixture");
    let debug_file = root.join("fixture.debug");
    let invocation_log = root.join("debuginfod.args");
    let cache = root.join("miniperf-cache");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(
        &source,
        "__attribute__((noinline)) int debuginfod_fixture(int value) {\n\
             return value * 19 + 7;\n\
         }\n\
         int main(void) { return debuginfod_fixture(2) == 45 ? 0 : 1; }\n",
    )
    .unwrap();
    run(Command::new("cc")
        .args(["-g", "-O0", "-fno-pie", "-no-pie"])
        .arg(&source)
        .arg("-o")
        .arg(&binary));
    run(Command::new("objcopy")
        .arg("--only-keep-debug")
        .arg(&binary)
        .arg(&debug_file));
    run(Command::new("objcopy").arg("--strip-debug").arg(&binary));

    let bytes = fs::read(&binary).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    assert!(object.gnu_debuglink().unwrap().is_none());
    let build_id = object.build_id().unwrap().expect("fixture has no build ID");
    let build_id_hex = hex(build_id);
    let mapped_start = object
        .segments()
        .filter(|segment| segment.file_range().0 == 0)
        .map(|segment| segment.address())
        .min()
        .expect("fixture has no segment mapped from offset zero");
    let address = object
        .symbols()
        .find(|symbol| symbol.name() == Ok("debuginfod_fixture"))
        .expect("fixture symbol is missing")
        .address();

    let client = bin_dir.join("debuginfod-find");
    fs::write(
        &client,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_DEBUGINFOD_LOG\"\nprintf '%s\\n' \"$FAKE_DEBUGINFOD_FILE\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&client).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&client, permissions).unwrap();

    std::env::set_var("PATH", &bin_dir);
    std::env::set_var("DEBUGINFOD_URLS", "https://debuginfod.invalid");
    std::env::set_var("MINIPERF_DEBUGINFOD", "1");
    std::env::set_var("FAKE_DEBUGINFOD_FILE", &debug_file);
    std::env::set_var("FAKE_DEBUGINFOD_LOG", &invocation_log);

    let downloaded_frames = resolve_fixture(&binary, &cache, mapped_start, address);
    assert!(
        downloaded_frames
            .iter()
            .any(|frame| frame.function.contains("debuginfod_fixture") && frame.line.is_some()),
        "downloaded frames: {downloaded_frames:?}"
    );
    assert_eq!(
        fs::read_to_string(&invocation_log).unwrap(),
        format!("debuginfo\n{build_id_hex}\n")
    );
    let cached = cache.join("buildid").join(&build_id_hex).join("debuginfo");
    assert_eq!(fs::read(&cached).unwrap(), fs::read(&debug_file).unwrap());

    fs::remove_file(&client).unwrap();
    std::env::remove_var("MINIPERF_DEBUGINFOD");
    std::env::remove_var("DEBUGINFOD_URLS");
    let cached_frames = resolve_fixture(&binary, &cache, mapped_start, address);
    assert!(
        cached_frames
            .iter()
            .any(|frame| frame.function.contains("debuginfod_fixture") && frame.line.is_some()),
        "cached frames: {cached_frames:?}"
    );

    fs::remove_dir_all(root).unwrap();
}
