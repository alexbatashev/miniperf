use anyhow::Result;
use mperf_data::{CallFrame, Event, IPCMessage, RecordInfo};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use pmu::{Counter, Process};

const SIZE_16MB: usize = 16 * 1024 * 1024;

use crate::{event_dispatcher::EventDispatcher, utils::counter_to_event_ty, Scenario};

pub async fn do_record(
    scenario: Scenario,
    output_directory: &Path,
    pid: Option<u32>,
    command: Vec<String>,
) -> Result<()> {
    println!("Record profile with {scenario:?} scenario");

    let json_command = if !command.is_empty() {
        Some(command.clone())
    } else {
        None
    };

    let info = RecordInfo {
        scenario,
        command: json_command,
        pid,
    };

    {
        let mut info_file = File::create(output_directory.join("info.json"))?;
        serde_json::to_writer(&mut info_file, &info)?;
    }

    let (dispatcher, join_handle) = EventDispatcher::new(output_directory);

    match scenario {
        Scenario::Snapshot => snapshot(dispatcher.clone(), &command)?,
        Scenario::Roofline => roofline(dispatcher.clone(), &command).await?,
    };

    drop(dispatcher);

    join_handle.join().await;

    Ok(())
}

fn snapshot(dispatcher: Arc<EventDispatcher>, command: &[String]) -> Result<()> {
    let process = Process::new(command, &[])?;

    let driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
            Counter::StalledCyclesBackend,
            Counter::StalledCyclesFrontend,
            Counter::CpuClock,
            Counter::CpuMigrations,
            Counter::PageFaults,
            Counter::ContextSwitches,
        ])
        .process(&process)
        .build()?;

    driver.start(move |sample| {
        let unique_id = dispatcher.unique_id();
        let callstack = sample.callstack.into_iter().map(CallFrame::IP).collect();
        let event = Event {
            unique_id,
            correlation_id: sample.event_id as u128,
            parent_id: 0,
            ty: counter_to_event_ty(&sample.counter),
            thread_id: sample.tid,
            process_id: sample.pid,
            time_enabled: sample.time_enabled,
            time_running: sample.time_running,
            value: sample.value,
            timestamp: sample.time,
            callstack,
        };

        dispatcher.publish_event_sync(event);
    })?;
    process.cont();
    process.wait()?;
    driver.stop()?;

    Ok(())
}

fn get_exe_dir() -> std::io::Result<PathBuf> {
    let mut exe_path = std::env::current_exe()?;
    exe_path.pop();
    Ok(exe_path)
}

async fn roofline(dispatcher: Arc<EventDispatcher>, command: &[String]) -> Result<()> {
    let exe_path = get_exe_dir()?.to_str().unwrap().to_string();

    // FIXME make this platform independent
    let ld_path = match std::env::var("LD_LIBRARY_PATH") {
        Ok(path) => format!("{}:{}:{}/../lib", path, exe_path, exe_path),
        Err(_) => format!("{}:{}/../lib", exe_path, exe_path),
    };

    println!(
        "Run 1: collecting performance data for '{}'",
        command.join(" ")
    );

    let (pipe_name, task) =
        create_shmem_pipe(command[0].split("/").last().unwrap(), dispatcher.clone())?;

    let process = Process::new(
        command,
        &[
            ("MPERF_COLLECTOR_SHMEM_ID".to_string(), pipe_name.clone()),
            ("LD_LIBRARY_PATH".to_string(), ld_path.clone()),
            ("MPERF_COLLECTOR_ENABLED".to_string(), "1".to_string()),
        ],
    )?;

    let driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
        ])
        .process(&process)
        .build()?;

    let roofline_dispatcher = dispatcher.clone();

    driver.start(move |sample| {
        let unique_id = dispatcher.unique_id();
        let callstack = sample.callstack.into_iter().map(CallFrame::IP).collect();
        let event = Event {
            unique_id,
            correlation_id: sample.event_id as u128,
            parent_id: 0,
            ty: counter_to_event_ty(&sample.counter),
            thread_id: sample.tid,
            process_id: sample.pid,
            time_enabled: sample.time_enabled,
            time_running: sample.time_running,
            value: sample.value,
            timestamp: sample.time,
            callstack,
        };

        dispatcher.publish_event_sync(event);
    })?;

    process.cont();
    process.wait()?;
    driver.stop()?;
    task.await?;

    println!(
        "Run 2: collecting loop statistics for '{}'",
        command.join(" ")
    );

    let (pipe_name, task) =
        create_shmem_pipe(command[0].split("/").last().unwrap(), roofline_dispatcher)?;

    let process = Process::new(
        command,
        &[
            ("MPERF_COLLECTOR_SHMEM_ID".to_string(), pipe_name.clone()),
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

    Ok(())
}

fn create_shmem_pipe(
    prefix: &str,
    roofline_dispatcher: Arc<EventDispatcher>,
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

    let rx = shmem::proc_channel::Receiver::<IPCMessage>::new(&pipe_name, SIZE_16MB)?;

    let task = tokio::spawn(async move {
        let mut strings = HashMap::<u64, u64>::new();

        while let Some(message) = rx.recv().await {
            match message {
                IPCMessage::String(string) => {
                    let id = roofline_dispatcher.string_id_async(&string.value).await;
                    strings.insert(string.key, id);
                }
                IPCMessage::Event(mut event) => {
                    for stack in event.callstack.iter_mut() {
                        if let CallFrame::Location(loc) = stack {
                            loc.function_name = strings
                                .get(&(loc.function_name as u64))
                                .cloned()
                                .unwrap_or_default()
                                as u128;
                            loc.file_name = strings
                                .get(&(loc.file_name as u64))
                                .cloned()
                                .unwrap_or_default()
                                as u128;
                        }
                    }

                    roofline_dispatcher.publish_event(event).await;
                }
            }
        }
    });

    Ok((pipe_name, task))
}
