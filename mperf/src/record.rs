use anyhow::Result;
use mperf_data::{CallFrame, Event, IPCMessage, RecordInfo};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

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
    let pipe_name = format!(
        "/{}{}",
        command[0].split("/").last().unwrap(),
        std::process::id()
    );

    let roofline_dispatcher = dispatcher.clone();

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

    let process = Process::new(
        command,
        &[
            ("MPERF_COLLECTOR_SHMEM_ID".to_string(), pipe_name.clone()),
            ("LD_LIBRARY_PATH".to_string(), ld_path.clone()),
            ("MPERF_COLLECTOR_ENABLED".to_string(), "1".to_string()),
        ],
    )?;

    let rx = shmem::proc_channel::Receiver::<IPCMessage>::new(&pipe_name, SIZE_16MB)?;

    let driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
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

    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    break;
                }
                Ok(message) = rx.recv() => {
                    match message {
                        IPCMessage::String(string) => {
                            roofline_dispatcher.string_id_async(&string.value).await;
                        }
                        IPCMessage::Event(event) => {
                            roofline_dispatcher.publish_event(event).await;
                        }
                    }
                }
            }
        }

        while !rx.empty() {
            if let Ok(message) = rx.recv_sync() {
                match message {
                    IPCMessage::String(string) => {
                        roofline_dispatcher.string_id_async(&string.value).await;
                    }
                    IPCMessage::Event(event) => {
                        roofline_dispatcher.publish_event(event).await;
                    }
                }
            }
        }
    });

    process.cont();
    process.wait()?;
    driver.stop()?;

    println!(
        "Run 2: collecting loop statistics for '{}'",
        command.join(" ")
    );

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

    tokio::time::sleep(tokio::time::Duration::from_millis(5000)).await;
    cancel_tx.send(true)?;
    task.await?;

    Ok(())
}
