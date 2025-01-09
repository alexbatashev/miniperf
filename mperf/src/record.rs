use anyhow::Result;
use mperf_data::{Event, EventType, RecordInfo};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

use pmu::{Counter, Process};

use crate::{event_dispatcher::EventDispatcher, Scenario};

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
        let name = dispatcher.string_id(sample.counter.name());
        let event = Event {
            unique_id,
            correlation_id: sample.event_id,
            parent_id: 0,
            name,
            ty: EventType::PMU,
            thread_id: sample.tid,
            process_id: sample.pid,
            time_enabled: sample.time_enabled,
            time_running: sample.time_running,
            value: sample.value,
            timestamp: sample.time,
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
            (
                "MPERF_COLLECTOR_IDS_START".to_string(),
                "100000000000".to_string(),
            ),
            ("LD_LIBRARY_PATH".to_string(), ld_path.clone()),
            ("MPERF_COLLECTOR_ENABLED".to_string(), "1".to_string()),
        ],
    )?;

    let rx = shmem::proc_channel::Receiver::<Event>::new(&pipe_name, 8192)?;

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
        let name = dispatcher.string_id(sample.counter.name());
        let event = Event {
            unique_id,
            correlation_id: sample.event_id,
            parent_id: 0,
            name,
            ty: EventType::PMU,
            thread_id: sample.tid,
            process_id: sample.pid,
            time_enabled: sample.time_enabled,
            time_running: sample.time_running,
            value: sample.value,
            timestamp: sample.time,
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
                Ok(event) = rx.recv() => {
                    roofline_dispatcher.publish_event(event).await;
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
            (
                "MPERF_COLLECTOR_IDS_START".to_string(),
                "100000000000".to_string(),
            ),
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

    cancel_tx.send(true)?;
    task.await?;

    Ok(())
}
