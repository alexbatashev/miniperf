use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use mperf_data::{CallFrame, Event, IPCMessage, ProcMapEntry, RooflineInfo, ScenarioInfo};
use pmu::{Counter, Process, Record};

use crate::{
    Scenario, counter_selection::get_pmu_counters, event_dispatcher::EventDispatcher,
    utils::counter_to_event_ty,
};

mod calibrate;
mod qemu;

const SIZE_16MB: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum BackendKind {
    #[default]
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
        if scenario != Scenario::Roofline && self.backend != BackendKind::Compiler {
            anyhow::bail!("--roofline-backend is only valid with the roofline scenario");
        }
        if self.backend != BackendKind::Qemu && has_qemu_options {
            anyhow::bail!("--qemu, --qemu-plugin, and --qemu-arg require --roofline-backend qemu");
        }
        Ok(())
    }
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

struct CompilerBackend;

pub async fn record(
    options: &Options,
    dispatcher: Arc<EventDispatcher>,
    command: &[String],
    output_directory: &Path,
) -> Result<ScenarioInfo> {
    if command.is_empty() {
        anyhow::bail!("record roofline requires a command");
    }

    let backend: Box<dyn RooflineBackend> = match options.backend {
        BackendKind::Compiler => Box::new(CompilerBackend),
        BackendKind::Qemu => Box::new(qemu::QemuBackend::new(options)?),
    };
    backend.record(dispatcher, command, output_directory).await
}

pub(crate) fn calibrate_host() -> Result<mperf_data::RooflineCalibration> {
    calibrate::measure()
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

            Ok(ScenarioInfo::Roofline(RooflineInfo {
                backend: "compiler".to_string(),
                perf_pid: baseline.pid,
                counters: baseline.counters,
                inst_pid: process.pid(),
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
                callstack: sample.callstack.into_iter().map(CallFrame::IP).collect(),
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
