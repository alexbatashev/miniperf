use anyhow::Result;
use mperf_data::{Event, EventType, RecordInfo};
use std::{fs::File, path::Path, sync::Arc, thread, time::Duration};

use pmu::Counter;

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
        Scenario::Snapshot => snapshot(dispatcher.clone())?,
        Scenario::Roofline => {
            todo!("roofline is not implemented yet")
        }
    };

    join_handle.join().await;

    Ok(())
}

fn snapshot(dispatcher: Arc<EventDispatcher>) -> Result<()> {
    let driver = pmu::SamplingDriver::builder()
        .counters(&[
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
        ])
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
        };

        dispatcher.publish_event(event);
    })?;
    thread::sleep(Duration::from_secs(1));
    driver.stop()?;

    Ok(())
}
