use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use mperf_data::{CallFrame, Event, EventType, Location, RooflineInfo, ScenarioInfo};
use object::{Object, ObjectKind};
use pmu::Process;
use smallvec::smallvec;

use super::{
    BackendFuture, Options, ProfiledRun, RooflineBackend, monotonic_timestamp, profile_command,
};
use crate::event_dispatcher::EventDispatcher;

pub(super) struct QemuBackend {
    qemu: Option<PathBuf>,
    plugin: PathBuf,
    qemu_args: Vec<String>,
}

#[derive(Default, Debug, Eq, PartialEq)]
struct Counts {
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
    bytes_load: u64,
    bytes_store: u64,
    rvv_state_errors: u64,
    unclassified_instructions: u64,
}

impl QemuBackend {
    #[cfg(not(target_os = "linux"))]
    pub(super) fn new(_options: &Options) -> Result<Self> {
        anyhow::bail!("the QEMU roofline backend supports Linux hosts only");
    }

    #[cfg(target_os = "linux")]
    pub(super) fn new(options: &Options) -> Result<Self> {
        let plugin = options
            .qemu_plugin
            .clone()
            .or_else(|| std::env::var_os("MPERF_QEMU_PLUGIN").map(PathBuf::from))
            .unwrap_or_else(default_plugin_path);
        if !plugin.is_file() {
            anyhow::bail!(
                "QEMU roofline plugin '{}' was not found; build it with `cargo build -p miniperf-qemu-roofline` or pass --qemu-plugin",
                plugin.display()
            );
        }

        Ok(Self {
            qemu: options.qemu.clone(),
            plugin,
            qemu_args: options.qemu_args.clone(),
        })
    }

    fn qemu_for(&self, guest: &Path) -> Result<PathBuf> {
        if let Some(qemu) = &self.qemu {
            return resolve_executable(qemu);
        }
        if let Some(qemu) = std::env::var_os("MPERF_QEMU") {
            return resolve_executable(&PathBuf::from(qemu));
        }

        let data = std::fs::read(guest)
            .with_context(|| format!("read guest executable '{}'", guest.display()))?;
        let object = object::File::parse(data.as_slice())
            .with_context(|| format!("parse guest executable '{}'", guest.display()))?;
        if !matches!(object.kind(), ObjectKind::Executable | ObjectKind::Dynamic) {
            anyhow::bail!("QEMU roofline guest must be an executable ELF");
        }
        let binary = qemu_binary_for_architecture(object.architecture())
            .context("unsupported guest architecture for QEMU roofline")?;
        which::which(binary)
            .with_context(|| format!("could not find '{binary}'; pass --qemu explicitly"))
    }

    fn command(&self, qemu: &Path, guest: &[String], output: Option<&Path>) -> Result<Vec<String>> {
        let mut command = vec![qemu.to_string_lossy().to_string()];
        command.extend(self.qemu_args.iter().cloned());
        if let Some(output) = output {
            let output = output.to_string_lossy();
            if output.contains(',') {
                anyhow::bail!("QEMU plugin output path cannot contain a comma");
            }
            command.push("-plugin".to_string());
            command.push(format!("{},output={output}", self.plugin.to_string_lossy()));
        }
        command.extend(guest.iter().cloned());
        Ok(command)
    }
}

impl RooflineBackend for QemuBackend {
    fn record<'a>(
        &'a self,
        dispatcher: Arc<EventDispatcher>,
        guest: &'a [String],
        output_directory: &'a Path,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            let qemu = self.qemu_for(Path::new(&guest[0]))?;
            ensure_plugin_support(&qemu)?;
            let baseline_command = self.command(&qemu, guest, None)?;
            println!(
                "Run 1: collecting QEMU-hosted performance data for '{}'",
                guest.join(" ")
            );
            let baseline =
                profile_command(dispatcher.clone(), &baseline_command, Vec::new(), false).await?;
            publish_baseline_region(
                dispatcher.clone(),
                baseline.pid,
                baseline.start_ns,
                baseline.end_ns,
                &guest[0],
            )
            .await;

            let counts_path = output_directory.join("qemu-roofline.counts");
            let accounting_command = self.command(&qemu, guest, Some(&counts_path))?;
            println!(
                "Run 2: collecting QEMU instruction and memory accounting for '{}'",
                guest.join(" ")
            );
            let process = Process::new(&accounting_command, &[])?;
            process.cont();
            process.wait()?;
            let counts =
                parse_counts(&std::fs::read_to_string(&counts_path).with_context(|| {
                    format!(
                        "QEMU roofline plugin did not produce '{}'",
                        counts_path.display()
                    )
                })?)?;
            if counts.rvv_state_errors != 0 {
                anyhow::bail!(
                    "QEMU roofline could not read RVV state for {} executed vector instructions",
                    counts.rvv_state_errors
                );
            }
            if counts.unclassified_instructions != 0 {
                eprintln!(
                    "Warning: TMDL could not classify {} executed RISC-V instructions; Roofline operation totals are conservative",
                    counts.unclassified_instructions
                );
            }
            publish_accounting_region(
                dispatcher,
                process.pid(),
                monotonic_timestamp()?,
                &guest[0],
                &counts,
            )
            .await;

            Ok(qemu_info(baseline, process.pid()))
        })
    }
}

fn resolve_executable(executable: &Path) -> Result<PathBuf> {
    if executable.components().count() == 1 {
        which::which(executable)
            .with_context(|| format!("could not find QEMU executable '{}'", executable.display()))
    } else {
        Ok(executable.to_owned())
    }
}

fn ensure_plugin_support(qemu: &Path) -> Result<()> {
    let output = std::process::Command::new(qemu)
        .arg("--help")
        .output()
        .with_context(|| format!("run '{} --help'", qemu.display()))?;
    if !qemu_help_has_plugin(&output.stdout) && !qemu_help_has_plugin(&output.stderr) {
        anyhow::bail!(
            "QEMU executable '{}' has no TCG plugin support; use a QEMU user-mode build whose --help lists -plugin",
            qemu.display()
        );
    }
    Ok(())
}

fn qemu_help_has_plugin(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| line.trim_start().starts_with("-plugin"))
}

fn qemu_info(baseline: ProfiledRun, accounting_pid: i32) -> ScenarioInfo {
    ScenarioInfo::Roofline(RooflineInfo {
        backend: "qemu".to_string(),
        perf_pid: baseline.pid,
        counters: baseline.counters,
        inst_pid: accounting_pid,
    })
}

async fn publish_baseline_region(
    dispatcher: Arc<EventDispatcher>,
    pid: i32,
    start: u64,
    end: u64,
    guest: &str,
) {
    let id = publish_start(&dispatcher, pid, start, guest).await;
    publish_end(&dispatcher, pid, end, id).await;
}

async fn publish_accounting_region(
    dispatcher: Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    guest: &str,
    counts: &Counts,
) {
    let id = publish_start(&dispatcher, pid, timestamp, guest).await;
    for (ty, value) in [
        (EventType::RooflineBytesLoad, counts.bytes_load),
        (EventType::RooflineBytesStore, counts.bytes_store),
        (EventType::RooflineScalarIntOps, counts.scalar_int_ops),
        (EventType::RooflineScalarFloatOps, counts.scalar_float_ops),
        (EventType::RooflineScalarDoubleOps, counts.scalar_double_ops),
        (EventType::RooflineVectorIntOps, counts.vector_int_ops),
        (EventType::RooflineVectorFloatOps, counts.vector_float_ops),
        (EventType::RooflineVectorDoubleOps, counts.vector_double_ops),
    ] {
        dispatcher
            .publish_event(synthetic_event(
                &dispatcher,
                pid,
                timestamp,
                ty,
                0,
                id,
                value,
            ))
            .await;
    }
    publish_end(&dispatcher, pid, timestamp, id).await;
}

async fn publish_start(
    dispatcher: &Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    guest: &str,
) -> u128 {
    let id = dispatcher.unique_id();
    let function_name = dispatcher.string_id_async("[QEMU whole process]").await;
    let file_name = dispatcher.string_id_async(guest).await;
    let mut event = synthetic_event(
        dispatcher,
        pid,
        timestamp,
        EventType::RooflineLoopStart,
        id,
        0,
        0,
    );
    event.callstack = smallvec![CallFrame::Location(Location {
        function_name,
        file_name,
        line: 0,
    })];
    dispatcher.publish_event(event).await;
    id
}

async fn publish_end(
    dispatcher: &Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    correlation_id: u128,
) {
    dispatcher
        .publish_event(synthetic_event(
            dispatcher,
            pid,
            timestamp,
            EventType::RooflineLoopEnd,
            0,
            correlation_id,
            0,
        ))
        .await;
}

fn synthetic_event(
    dispatcher: &EventDispatcher,
    pid: i32,
    timestamp: u64,
    ty: EventType,
    unique_id: u128,
    parent_or_correlation_id: u128,
    value: u64,
) -> Event {
    let is_counter = matches!(
        ty,
        EventType::RooflineBytesLoad
            | EventType::RooflineBytesStore
            | EventType::RooflineScalarIntOps
            | EventType::RooflineScalarFloatOps
            | EventType::RooflineScalarDoubleOps
            | EventType::RooflineVectorIntOps
            | EventType::RooflineVectorFloatOps
            | EventType::RooflineVectorDoubleOps
    );
    Event {
        unique_id: if unique_id == 0 {
            dispatcher.unique_id()
        } else {
            unique_id
        },
        correlation_id: if is_counter {
            0
        } else {
            parent_or_correlation_id
        },
        parent_id: if is_counter {
            parent_or_correlation_id
        } else {
            0
        },
        ty,
        thread_id: 0,
        process_id: pid as u32,
        cpu: u32::MAX,
        time_enabled: 0,
        time_running: 0,
        value,
        timestamp,
        name: 0,
        callstack: smallvec![],
        user_regs: None,
        user_stack: Vec::new(),
    }
}

fn parse_counts(input: &str) -> Result<Counts> {
    let values = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (name, value) = line
                .split_once('=')
                .with_context(|| format!("invalid QEMU roofline count '{line}'"))?;
            value
                .parse::<u64>()
                .map(|value| (name, value))
                .with_context(|| format!("invalid QEMU roofline count '{line}'"))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    let get = |name| values.get(name).copied().unwrap_or_default();
    Ok(Counts {
        scalar_int_ops: get("scalar_int_ops"),
        scalar_float_ops: get("scalar_float_ops"),
        scalar_double_ops: get("scalar_double_ops"),
        vector_int_ops: get("vector_int_ops"),
        vector_float_ops: get("vector_float_ops"),
        vector_double_ops: get("vector_double_ops"),
        bytes_load: get("bytes_load"),
        bytes_store: get("bytes_store"),
        rvv_state_errors: get("rvv_state_errors"),
        unclassified_instructions: get("unclassified_instructions"),
    })
}

#[cfg(target_os = "linux")]
fn default_plugin_path() -> PathBuf {
    super::executable_directory()
        .unwrap_or_default()
        .join("libminiperf_qemu_roofline.so")
}

fn qemu_binary_for_architecture(architecture: object::Architecture) -> Option<&'static str> {
    match architecture {
        object::Architecture::Riscv64 => Some("qemu-riscv64"),
        object::Architecture::Riscv32 => Some("qemu-riscv32"),
        object::Architecture::X86_64 => Some("qemu-x86_64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_counts() {
        let counts = parse_counts(
            "scalar_int_ops=7\nscalar_double_ops=3\nvector_float_ops=2\nbytes_load=64\nbytes_store=32\nunclassified_instructions=5\n",
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                scalar_int_ops: 7,
                scalar_double_ops: 3,
                vector_float_ops: 2,
                bytes_load: 64,
                bytes_store: 32,
                unclassified_instructions: 5,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn maps_supported_guests_to_user_mode_qemu() {
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::Riscv64),
            Some("qemu-riscv64")
        );
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::Riscv32),
            Some("qemu-riscv32")
        );
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::X86_64),
            Some("qemu-x86_64")
        );
    }

    #[test]
    fn recognizes_qemu_plugin_help() {
        assert!(qemu_help_has_plugin(
            b"  -plugin [file=]file[,arg=<string>]\n"
        ));
        assert!(!qemu_help_has_plugin(b"  -cpu model\n"));
    }
}
