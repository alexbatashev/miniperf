use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use mperf_data::{
    CallFrame, Event, IPCMessage, ProcMapEntry, RooflineInfo, RooflineMethodInfo, ScenarioInfo,
};
use object::{Object, ObjectSymbol};
use pmu::{Counter, Process, Record};

use crate::{
    Scenario, counter_selection::get_pmu_counters, event_dispatcher::EventDispatcher,
    utils::counter_to_event_ty,
};

mod calibrate;
mod loops;
mod qemu;

const SIZE_16MB: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum BackendKind {
    #[default]
    Auto,
    Compiler,
    Qemu,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub backend: BackendKind,
    pub qemu: Option<PathBuf>,
    pub qemu_plugin: Option<PathBuf>,
    pub qemu_args: Vec<String>,
}

impl Options {
    pub fn validate_for(&self, scenario: Scenario) -> Result<()> {
        let has_qemu_options =
            self.qemu.is_some() || self.qemu_plugin.is_some() || !self.qemu_args.is_empty();
        if scenario != Scenario::Roofline && self.backend != BackendKind::Auto {
            anyhow::bail!("--roofline-backend is only valid with the roofline scenario");
        }
        if scenario != Scenario::Roofline && has_qemu_options {
            anyhow::bail!(
                "--qemu, --qemu-plugin, and --qemu-arg are only valid with the roofline scenario"
            );
        }
        if self.backend == BackendKind::Compiler && has_qemu_options {
            anyhow::bail!(
                "--qemu, --qemu-plugin, and --qemu-arg cannot be used with --roofline-backend compiler"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PerformanceSource {
    Native,
    Qemu,
}

struct SelectedMethod {
    backend: BackendKind,
    performance: PerformanceSource,
    executable: PathBuf,
    info: RooflineMethodInfo,
}

type BackendFuture<'a> = Pin<Box<dyn Future<Output = Result<ScenarioInfo>> + 'a>>;

trait RooflineBackend {
    fn record<'a>(
        &'a self,
        dispatcher: Arc<EventDispatcher>,
        command: &'a [String],
        output_directory: &'a Path,
    ) -> BackendFuture<'a>;
}

struct CompilerBackend {
    method: RooflineMethodInfo,
}

pub async fn record(
    options: &Options,
    dispatcher: Arc<EventDispatcher>,
    command: &[String],
    output_directory: &Path,
) -> Result<ScenarioInfo> {
    if command.is_empty() {
        anyhow::bail!("record roofline requires a command");
    }

    let selected = select_method(options, command)?;
    println!("Roofline method: {}", selected.info.reason);
    for warning in &selected.info.warnings {
        eprintln!("Warning: {warning}");
    }
    let mut resolved_command = command.to_vec();
    resolved_command[0] = selected.executable.to_string_lossy().into_owned();

    let backend: Box<dyn RooflineBackend> = match selected.backend {
        BackendKind::Auto => unreachable!("automatic Roofline method must resolve to a backend"),
        BackendKind::Compiler => Box::new(CompilerBackend {
            method: selected.info,
        }),
        BackendKind::Qemu => Box::new(qemu::QemuBackend::new(
            options,
            selected.performance == PerformanceSource::Native,
            selected.info,
        )?),
    };
    backend
        .record(dispatcher, &resolved_command, output_directory)
        .await
}

fn select_method(options: &Options, command: &[String]) -> Result<SelectedMethod> {
    let guest = inspect_guest(Path::new(&command[0]))?;
    let native = guest.architecture == host_architecture();
    let qemu_probe = qemu::probe(options, &guest.path);

    match options.backend {
        BackendKind::Qemu => {
            qemu_probe?;
            Ok(qemu_method(false, native, guest.path))
        }
        BackendKind::Compiler => {
            if !guest.compiler_instrumented {
                anyhow::bail!(
                    "compiler Roofline was requested, but '{}' does not contain complete miniperf loop instrumentation",
                    guest.path.display()
                );
            }
            Ok(compiler_method(false, guest.path))
        }
        BackendKind::Auto => {
            if qemu_probe.is_ok() && native {
                return Ok(qemu_method(true, native, guest.path));
            }
            if qemu_probe.is_ok() {
                anyhow::bail!(
                    "accurate Roofline performance is unavailable for cross-architecture executable '{}': QEMU can provide operation accounting but not native RISC-V timing; run the same mperf command on a compatible RISC-V host (explicit --roofline-backend qemu remains available for emulation diagnostics)",
                    guest.path.display()
                );
            }
            if guest.compiler_instrumented {
                return Ok(compiler_method(true, guest.path));
            }

            let qemu_error = qemu_probe.unwrap_err();
            anyhow::bail!(
                "no accurate Roofline method is available for '{}': QEMU accounting is unavailable ({qemu_error:#}) and the executable has no miniperf loop instrumentation; install a plugin-enabled QEMU and the miniperf QEMU plugin",
                guest.path.display()
            )
        }
    }
}

fn qemu_method(automatic: bool, native: bool, executable: PathBuf) -> SelectedMethod {
    let performance = if native {
        PerformanceSource::Native
    } else {
        PerformanceSource::Qemu
    };
    let (quality, reason, warnings) = if native {
        (
            "hybrid-binary-sampled-cache-model".to_string(),
            "native timing with QEMU operation accounting, shared-LLC traffic modeling, and dynamic binary loop discovery"
                .to_string(),
            vec![
                "per-loop throughput is published only when native timing has at most 10% estimated 95% sampling error; lower-confidence loops retain accounting but are not plotted".to_string(),
                "native timing and QEMU accounting come from separate executions".to_string(),
                "memory traffic is a deterministic host-LLC model, not a hardware memory-controller measurement".to_string(),
                "the cache model uses write-back stores without read-for-ownership traffic".to_string(),
            ],
        )
    } else {
        (
            "emulation-analysis".to_string(),
            "QEMU timing, operation accounting, and shared-LLC traffic modeling because the guest cannot execute natively"
                .to_string(),
            vec![
                "throughput is based on emulator time and must not be interpreted as guest-hardware performance".to_string(),
                "the Roofline UI point is currently whole-process; per-loop candidates and accounting are saved in qemu-roofline.loops.json".to_string(),
                "memory traffic is a deterministic host-LLC model, not a hardware memory-controller measurement".to_string(),
            ],
        )
    };
    SelectedMethod {
        backend: BackendKind::Qemu,
        performance,
        executable,
        info: RooflineMethodInfo {
            selection: if automatic { "auto" } else { "explicit" }.to_string(),
            accounting: "qemu".to_string(),
            performance: if native { "native" } else { "qemu" }.to_string(),
            traffic: "dram-model".to_string(),
            quality,
            reason,
            warnings,
        },
    }
}

fn compiler_method(automatic: bool, executable: PathBuf) -> SelectedMethod {
    SelectedMethod {
        backend: BackendKind::Compiler,
        performance: PerformanceSource::Native,
        executable,
        info: RooflineMethodInfo {
            selection: if automatic { "auto" } else { "explicit" }.to_string(),
            accounting: "compiler".to_string(),
            performance: "native".to_string(),
            traffic: "architectural".to_string(),
            quality: "compiler-instrumented".to_string(),
            reason: "native timing and detected miniperf compiler instrumentation".to_string(),
            warnings: vec![
                "the legacy compiler backend only reports loops transformed by the miniperf LLVM pass"
                    .to_string(),
            ],
        },
    }
}

struct GuestInfo {
    path: PathBuf,
    architecture: object::Architecture,
    compiler_instrumented: bool,
}

fn inspect_guest(path: &Path) -> Result<GuestInfo> {
    let path = if path.components().count() == 1 {
        which::which(path)
            .with_context(|| format!("could not find executable '{}'", path.display()))?
    } else {
        path.to_owned()
    };
    let data = std::fs::read(&path)
        .with_context(|| format!("read Roofline executable '{}'", path.display()))?;
    let object = object::File::parse(data.as_slice())
        .with_context(|| format!("parse Roofline executable '{}'", path.display()))?;
    let instrumentation_symbols = object
        .symbols()
        .chain(object.dynamic_symbols())
        .filter_map(|symbol| symbol.name().ok())
        .filter(|name| {
            matches!(
                *name,
                "mperf_roofline_internal_notify_loop_begin"
                    | "mperf_roofline_internal_notify_loop_end"
                    | "mperf_roofline_internal_notify_loop_stats"
            )
        })
        .collect::<std::collections::HashSet<_>>();
    let compiler_instrumented = instrumentation_symbols.len() == 3;
    Ok(GuestInfo {
        path,
        architecture: object.architecture(),
        compiler_instrumented,
    })
}

fn host_architecture() -> object::Architecture {
    #[cfg(target_arch = "x86_64")]
    return object::Architecture::X86_64;
    #[cfg(target_arch = "riscv64")]
    return object::Architecture::Riscv64;
    #[cfg(target_arch = "riscv32")]
    return object::Architecture::Riscv32;
    #[cfg(target_arch = "aarch64")]
    return object::Architecture::Aarch64;
    #[allow(unreachable_code)]
    object::Architecture::Unknown
}

pub(crate) fn calibrate_host() -> Result<mperf_data::RooflineCalibration> {
    calibrate::measure()
}

pub(crate) fn uses_native_performance(options: &Options, command: &[String]) -> Result<bool> {
    if command.is_empty() {
        anyhow::bail!("record roofline requires a command");
    }
    Ok(select_method(options, command)?.performance == PerformanceSource::Native)
}

impl RooflineBackend for CompilerBackend {
    fn record<'a>(
        &'a self,
        dispatcher: Arc<EventDispatcher>,
        command: &'a [String],
        _output_directory: &'a Path,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            let exe_path = executable_directory()?.to_string_lossy().to_string();
            let ld_path = match std::env::var("LD_LIBRARY_PATH") {
                Ok(path) => format!("{path}:{exe_path}:{exe_path}/../lib"),
                Err(_) => format!("{exe_path}:{exe_path}/../lib"),
            };

            println!(
                "Run 1: collecting performance data for '{}'",
                command.join(" ")
            );
            let baseline = profile_command(
                dispatcher.clone(),
                command,
                vec![
                    ("LD_LIBRARY_PATH".to_string(), ld_path.clone()),
                    ("MPERF_COLLECTOR_ENABLED".to_string(), "1".to_string()),
                ],
                true,
            )
            .await?;

            println!(
                "Run 2: collecting compiler-instrumented loop statistics for '{}'",
                command.join(" ")
            );
            let (pipe_name, task) = create_shmem_pipe(command_name(command), dispatcher)?;
            let process = Process::new(
                command,
                &[
                    ("MPERF_COLLECTOR_SHMEM_ID".to_string(), pipe_name),
                    ("LD_LIBRARY_PATH".to_string(), ld_path),
                    ("MPERF_COLLECTOR_ENABLED".to_string(), "1".to_string()),
                    (
                        "MPERF_COLLECTOR_ROOFLINE_INSTRUMENTED".to_string(),
                        "1".to_string(),
                    ),
                ],
            )?;
            process.cont();
            process.wait()?;
            task.await?;
            ensure_process_success(&process, "compiler Roofline accounting run")?;

            Ok(ScenarioInfo::Roofline(RooflineInfo {
                backend: "compiler".to_string(),
                perf_pid: baseline.pid,
                counters: baseline.counters,
                inst_pid: process.pid(),
                method: Some(Box::new(self.method.clone())),
            }))
        })
    }
}

struct ProfiledRun {
    pid: i32,
    start_ns: u64,
    end_ns: u64,
    counters: Vec<(mperf_data::EventType, String)>,
}

async fn profile_command(
    dispatcher: Arc<EventDispatcher>,
    command: &[String],
    mut env: Vec<(String, String)>,
    receive_collector_events: bool,
) -> Result<ProfiledRun> {
    let collector = if receive_collector_events {
        let (pipe_name, task) = create_shmem_pipe(command_name(command), dispatcher.clone())?;
        env.push(("MPERF_COLLECTOR_SHMEM_ID".to_string(), pipe_name));
        Some(task)
    } else {
        None
    };
    let process = Process::new(command, &env)?;
    let counters = get_pmu_counters(Scenario::Roofline);
    let mut driver = pmu::SamplingDriverBuilder::new()
        .counters(&counters)
        .process(&process)
        .build()?;
    let sample_dispatcher = dispatcher;
    driver.start(Arc::new(move |record| match record {
        Record::Sample(sample) => {
            let name = if let Counter::Custom(name) = &sample.counter {
                sample_dispatcher.string_id(name)
            } else {
                0
            };
            let mut callstack = smallvec::smallvec![CallFrame::IP(sample.ip)];
            callstack.extend(
                sample
                    .callstack
                    .into_iter()
                    .filter(|address| *address != sample.ip)
                    .map(CallFrame::IP),
            );
            sample_dispatcher.publish_event_sync(Event {
                unique_id: uuid::Uuid::now_v7().as_u128(),
                correlation_id: sample.event_id,
                parent_id: 0,
                ty: counter_to_event_ty(&sample.counter),
                thread_id: sample.tid,
                process_id: sample.pid,
                cpu: sample.cpu,
                time_enabled: sample.time_enabled,
                time_running: sample.time_running,
                value: sample.value,
                timestamp: sample.time,
                name,
                callstack,
                user_regs: sample.user_regs.map(|regs| mperf_data::UserRegs {
                    abi: regs.abi,
                    mask: regs.mask,
                    values: regs.values,
                }),
                user_stack: sample.user_stack,
            });
        }
        Record::ProcAddr(addr) => {
            sample_dispatcher.publish_proc_map_sync(ProcMapEntry {
                filename: addr.filename,
                address: addr.addr as usize,
                size: addr.len as usize,
                offset: addr.pgoff as usize,
                pid: addr.pid,
            });
        }
    }))?;

    let start_ns = monotonic_timestamp()?;
    process.cont();
    process.wait()?;
    let end_ns = monotonic_timestamp()?;
    driver.stop()?;
    if let Some(task) = collector {
        task.await?;
    }
    ensure_process_success(&process, "Roofline performance run")?;

    Ok(ProfiledRun {
        pid: process.pid(),
        start_ns,
        end_ns,
        counters: counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
    })
}

fn ensure_process_success(process: &Process, description: &str) -> Result<()> {
    match process.exit_code() {
        Some(0) => Ok(()),
        Some(code) => anyhow::bail!("{description} exited with status {code}"),
        None => anyhow::bail!("{description} finished without an observable exit status"),
    }
}

fn create_shmem_pipe(
    prefix: &str,
    dispatcher: Arc<EventDispatcher>,
) -> Result<(String, tokio::task::JoinHandle<()>), std::io::Error> {
    let pipe_name = format!(
        "/{}{}{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    let receiver = shmem::proc_channel::Receiver::<IPCMessage>::new(&pipe_name, SIZE_16MB)?;
    let task = tokio::spawn(async move {
        let mut strings = HashMap::<u128, u128>::new();
        while let Some(message) = receiver.recv().await {
            match message {
                IPCMessage::String(string) => {
                    let id = dispatcher.string_id_async(&string.value).await;
                    strings.insert(string.key, id);
                }
                IPCMessage::Event(mut event) => {
                    for stack in event.callstack.iter_mut() {
                        if let CallFrame::Location(location) = stack {
                            location.function_name = strings
                                .get(&location.function_name)
                                .copied()
                                .unwrap_or_default();
                            location.file_name = strings
                                .get(&location.file_name)
                                .copied()
                                .unwrap_or_default();
                        }
                    }
                    dispatcher.publish_event(event).await;
                }
            }
        }
    });
    Ok((pipe_name, task))
}

fn executable_directory() -> std::io::Result<PathBuf> {
    let mut path = std::env::current_exe()?;
    path.pop();
    Ok(path)
}

fn command_name(command: &[String]) -> &str {
    Path::new(&command[0])
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mperf")
}

fn monotonic_timestamp() -> Result<u64> {
    let mut timestamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut timestamp) } != 0 {
        return Err(std::io::Error::last_os_error()).context("read monotonic clock");
    }
    Ok((timestamp.tv_sec as u64) * 1_000_000_000 + timestamp.tv_nsec as u64)
}
