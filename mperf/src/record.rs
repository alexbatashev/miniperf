use anyhow::{Context, Result};
use mperf_data::{
    CallFrame, Event, IPCMessage, ProcMapEntry, RecordInfo, RooflineInfo, ScenarioInfo,
};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use pmu::{Counter, Process, Record};

const SIZE_16MB: usize = 16 * 1024 * 1024;

use crate::{
    counter_selection::get_pmu_counters, event_dispatcher::EventDispatcher,
    postprocess::perform_postprocessing, utils::counter_to_event_ty, Scenario,
};

#[cfg(target_os = "macos")]
const VM_PROT_EXECUTE: i32 = 0x4;

pub async fn do_record(
    scenario: Scenario,
    output_directory: &Path,
    pid: Option<u32>,
    command: Vec<String>,
) -> Result<()> {
    println!("Record profile with {scenario:?} scenario");

    let (dispatcher, join_handle) = EventDispatcher::new(output_directory);

    let info = match scenario {
        Scenario::Snapshot => snapshot(dispatcher.clone(), pid, &command)?,
        Scenario::Roofline => roofline(dispatcher.clone(), &command).await?,
        Scenario::TMA => topdown(dispatcher.clone(), &command)?,
    };

    drop(dispatcher);

    join_handle.join().await;

    let json_command = if !command.is_empty() {
        Some(command.clone())
    } else {
        None
    };

    let (cpu_vendor, cpu_model) = pmu::host_cpu_description();

    let cores = pmu::host_core_clusters()
        .into_iter()
        .map(|c| mperf_data::CoreCluster {
            family_id: c.family_id,
            name: c.name,
            cpus: c.cpus,
        })
        .collect();

    let ri = RecordInfo {
        format_version: mperf_data::CURRENT_FORMAT_VERSION,
        scenario,
        command: json_command,
        cpu_model,
        cpu_vendor,
        cores,
        scenario_info: info,
    };

    {
        let mut info_file = File::create(output_directory.join("info.json"))?;
        serde_json::to_writer(&mut info_file, &ri)?;
    }

    println!("Postprocessing...");
    kdam::term::init(false);
    kdam::term::hide_cursor()?;

    let pb = kdam::tqdm!(total = 100);
    perform_postprocessing(output_directory, pb).await?;

    kdam::term::show_cursor()?;

    Ok(())
}

fn snapshot(
    dispatcher: Arc<EventDispatcher>,
    pid: Option<u32>,
    command: &[String],
) -> Result<ScenarioInfo> {
    if pid.is_none() && command.is_empty() {
        anyhow::bail!("record snapshot requires a command or --pid");
    }

    let process = if pid.is_none() {
        Some(Process::new(command, &[])?)
    } else {
        None
    };

    let counters = get_pmu_counters(Scenario::Snapshot);

    let mut builder = pmu::SamplingDriverBuilder::new().counters(&counters);
    if let Some(process) = &process {
        builder = builder.process(process);
    } else if let Some(pid) = pid {
        builder = builder.pid(pid as i32);
    }
    let mut driver = builder.build()?;
    let recorded_pid = pid.unwrap_or_else(|| process.as_ref().unwrap().pid() as u32) as i32;
    // On macOS Process::new returns an already-exec'd, suspended child, so its
    // dyld mappings are available before the first instruction is profiled.
    // Attached processes are already live on every platform.
    if cfg!(target_os = "macos") || pid.is_some() {
        publish_process_maps(dispatcher.clone(), recorded_pid);
    }

    let sample_dispatcher = dispatcher.clone();
    driver.start(Arc::new(move |record| {
        match record {
            Record::Sample(sample) => {
                let unique_id = uuid::Uuid::now_v7().as_u128();
                let callstack = sample.callstack.into_iter().map(CallFrame::IP).collect();
                let name = if let Counter::Custom(name) = &sample.counter {
                    sample_dispatcher.string_id(name)
                } else {
                    0
                };
                let event = Event {
                    unique_id,
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
                };

                sample_dispatcher.publish_event_sync(event);
            }
            Record::ProcAddr(addr) => {
                let entry = ProcMapEntry {
                    filename: addr.filename,
                    address: addr.addr as usize,
                    size: addr.len as usize,
                    offset: addr.pgoff as usize,
                    pid: addr.pid,
                };

                sample_dispatcher.publish_proc_map_sync(entry);
            }
        };
    }))?;
    if let Some(process) = &process {
        process.cont();
        std::thread::sleep(std::time::Duration::from_millis(20));
        publish_process_maps(dispatcher.clone(), recorded_pid);
        process.wait()?;
    } else if let Some(pid) = pid {
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    driver.stop()?;

    Ok(ScenarioInfo::Snapshot(mperf_data::SnapshotInfo {
        pid: recorded_pid,
        counters: counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
    }))
}

fn publish_process_maps(dispatcher: Arc<EventDispatcher>, pid: i32) {
    #[cfg(target_os = "macos")]
    if let Ok(images) = proc_maps::mac_maps::get_dyld_info(pid as proc_maps::Pid) {
        if !images.is_empty() {
            let mut link_bases = HashMap::<PathBuf, Option<u64>>::new();
            for image in images {
                // proc-maps exposes every LC_SEGMENT_64 command, including
                // __PAGEZERO. It is not mapped and its multi-gigabyte virtual
                // span would falsely claim most user addresses during symbol
                // lookup.
                if !macos_segment_is_executable(image.segment.vmsize, image.segment.initprot) {
                    continue;
                }
                let link_base = *link_bases
                    .entry(image.filename.clone())
                    .or_insert_with(|| mach_o_text_address(&image.filename));
                let link_address = link_base
                    .and_then(|base| {
                        let slide = (image.address as u64).checked_sub(base)?;
                        image.segment.vmaddr.checked_sub(slide)
                    })
                    .unwrap_or(image.segment.fileoff);
                let entry = ProcMapEntry {
                    filename: image.filename.to_string_lossy().to_string(),
                    address: image.segment.vmaddr as usize,
                    size: image.segment.vmsize as usize,
                    // For Mach-O, addr2line consumes link-time virtual
                    // addresses. Store the unslid segment VM address here so
                    // `runtime - address + offset` reconstructs that address.
                    offset: link_address as usize,
                    pid: pid as u32,
                };
                dispatcher.publish_proc_map_sync(entry);
            }
            return;
        }
    }

    let Ok(maps) = proc_maps::get_process_maps(pid as proc_maps::Pid) else {
        return;
    };

    for map in maps {
        if !map.is_exec() {
            continue;
        }
        let Some(filename) = map.filename() else {
            continue;
        };
        let entry = ProcMapEntry {
            filename: filename.to_string_lossy().to_string(),
            address: map.start(),
            size: map.size(),
            offset: 0,
            pid: pid as u32,
        };
        dispatcher.publish_proc_map_sync(entry);
    }
}

#[cfg(target_os = "macos")]
fn mach_o_text_address(path: &Path) -> Option<u64> {
    use object::{Object, ObjectSegment};

    let data = std::fs::read(path).ok()?;
    let object = object::File::parse(data.as_slice()).ok()?;
    object
        .segments()
        .find(|segment| segment.name().ok().flatten() == Some("__TEXT"))
        .map(|segment| segment.address())
}

#[cfg(target_os = "macos")]
fn macos_segment_is_executable(size: u64, initial_protection: i32) -> bool {
    size > 0 && initial_protection & VM_PROT_EXECUTE != 0
}

fn get_exe_dir() -> std::io::Result<PathBuf> {
    let mut exe_path = std::env::current_exe()?;
    exe_path.pop();
    Ok(exe_path)
}

async fn roofline(dispatcher: Arc<EventDispatcher>, command: &[String]) -> Result<ScenarioInfo> {
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

    let counters = get_pmu_counters(Scenario::Roofline);

    let mut driver = pmu::SamplingDriverBuilder::new()
        .counters(&counters)
        .process(&process)
        .build()?;

    let roofline_dispatcher = dispatcher.clone();

    driver.start(Arc::new(move |record| {
        match record {
            Record::Sample(sample) => {
                let unique_id = uuid::Uuid::now_v7().as_u128();
                let callstack = sample.callstack.into_iter().map(CallFrame::IP).collect();
                let name = if let Counter::Custom(name) = &sample.counter {
                    dispatcher.string_id(name)
                } else {
                    0
                };
                let event = Event {
                    unique_id,
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
                };

                dispatcher.publish_event_sync(event);
            }
            Record::ProcAddr(addr) => {
                let entry = ProcMapEntry {
                    filename: addr.filename,
                    address: addr.addr as usize,
                    size: addr.len as usize,
                    offset: addr.pgoff as usize,
                    pid: addr.pid,
                };

                dispatcher.publish_proc_map_sync(entry);
            }
        };
    }))?;

    process.cont();
    process.wait()?;
    driver.stop()?;
    task.await?;

    let perf_pid = process.pid();

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

    let inst_pid = process.pid();

    Ok(ScenarioInfo::Roofline(RooflineInfo {
        perf_pid,
        counters: counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
        inst_pid,
    }))
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
        let mut strings = HashMap::<u128, u128>::new();

        while let Some(message) = rx.recv().await {
            match message {
                IPCMessage::String(string) => {
                    let id = roofline_dispatcher.string_id_async(&string.value).await;
                    strings.insert(string.key, id);
                }
                IPCMessage::Event(mut event) => {
                    for stack in event.callstack.iter_mut() {
                        if let CallFrame::Location(loc) = stack {
                            loc.function_name =
                                strings.get(&loc.function_name).cloned().unwrap_or_default()
                                    as u128;
                            loc.file_name =
                                strings.get(&loc.file_name).cloned().unwrap_or_default() as u128;
                        }
                    }

                    roofline_dispatcher.publish_event(event).await;
                }
            }
        }
    });

    Ok((pipe_name, task))
}

fn topdown(dispatcher: Arc<EventDispatcher>, command: &[String]) -> Result<ScenarioInfo> {
    let scenario = pmu::host_tma_scenario().context("TMA is not supported on this CPU")?;
    let process = Process::new(command, &[])?;
    let counters = get_pmu_counters(Scenario::TMA);

    let mut driver = pmu::SamplingDriverBuilder::new()
        .counters(&counters)
        .process(&process)
        .build()?;
    let recorded_pid = process.pid();
    if cfg!(target_os = "macos") {
        publish_process_maps(dispatcher.clone(), recorded_pid);
    }

    let sample_dispatcher = dispatcher.clone();
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
                name,
                timestamp: sample.time,
                callstack: sample.callstack.into_iter().map(CallFrame::IP).collect(),
                user_regs: sample.user_regs.map(|regs| mperf_data::UserRegs {
                    abi: regs.abi,
                    mask: regs.mask,
                    values: regs.values,
                }),
                user_stack: sample.user_stack,
            });
        }
        Record::ProcAddr(addr) => sample_dispatcher.publish_proc_map_sync(ProcMapEntry {
            filename: addr.filename,
            address: addr.addr as usize,
            size: addr.len as usize,
            offset: addr.pgoff as usize,
            pid: addr.pid,
        }),
    }))?;

    process.cont();
    std::thread::sleep(std::time::Duration::from_millis(20));
    publish_process_maps(dispatcher, recorded_pid);
    process.wait()?;
    driver.stop()?;

    Ok(ScenarioInfo::TMA(mperf_data::TMAInfo {
        pid: recorded_pid,
        counters: counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
        metrics: scenario.metrics,
        constants: scenario.constants,
        ui: scenario.ui,
    }))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{mach_o_text_address, macos_segment_is_executable, VM_PROT_EXECUTE};

    #[test]
    fn finds_link_time_text_address_in_current_mach_o() {
        let executable = std::env::current_exe().unwrap();
        assert!(mach_o_text_address(&executable).is_some());
    }

    #[test]
    fn rejects_non_executable_mach_o_segments() {
        assert!(!macos_segment_is_executable(0x1_0000_0000, 0));
        assert!(!macos_segment_is_executable(0x1000, 1));
        assert!(!macos_segment_is_executable(0, VM_PROT_EXECUTE));
        assert!(macos_segment_is_executable(0x1000, VM_PROT_EXECUTE));
    }
}
