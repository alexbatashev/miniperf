use anyhow::{Context, Result};
use mperf_data::{CallFrame, CpuClockSource, Event, ProcMapEntry, RecordInfo, ScenarioInfo};
use std::{fs::File, path::Path, sync::Arc};

#[cfg(target_os = "macos")]
use std::{collections::HashMap, path::PathBuf};

use pmu::{Counter, Process, Record};

use crate::{
    Scenario,
    counter_selection::{get_pmu_counters, get_tma_counter_groups},
    event_dispatcher::EventDispatcher,
    postprocess::perform_postprocessing,
    roofline,
    utils::counter_to_event_ty,
};

#[cfg(target_os = "macos")]
const VM_PROT_EXECUTE: i32 = 0x4;

pub async fn do_record(
    scenario: Scenario,
    output_directory: &Path,
    pid: Option<u32>,
    command: Vec<String>,
    roofline_options: roofline::Options,
) -> Result<()> {
    println!("Record profile with {scenario:?} scenario");

    let cpu_info = if scenario == Scenario::Roofline
        && roofline::uses_native_performance(&roofline_options, &command)?
    {
        println!("Calibrating host Roofline ceilings...");
        let calibration = roofline::calibrate_host()?;
        println!(
            "Host ceilings: {:.2} GFLOP/s FP64, {:.2} GB/s memory ({} Rayon threads)",
            calibration.fp64_gflops, calibration.memory_gbytes_per_second, calibration.threads
        );
        mperf_data::CpuInfo {
            roofline_calibration: Some(Box::new(calibration)),
        }
    } else {
        if scenario == Scenario::Roofline {
            println!(
                "Skipping host Roofline calibration: the selected method does not measure native performance"
            );
        }
        mperf_data::CpuInfo::default()
    };

    let (dispatcher, join_handle) = EventDispatcher::new(output_directory);

    let info = match scenario {
        Scenario::Snapshot => snapshot(dispatcher.clone(), pid, &command)?,
        Scenario::Roofline => {
            roofline::record(
                &roofline_options,
                dispatcher.clone(),
                &command,
                output_directory,
            )
            .await?
        }
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
        sampling_frequency_hz: Some(pmu::DEFAULT_SAMPLE_FREQUENCY_HZ),
        cpu_clock_source: Some(if cfg!(any(target_os = "macos", target_os = "linux")) {
            CpuClockSource::SampledOccupancy
        } else {
            CpuClockSource::CounterDelta
        }),
        logical_cpu_count: host_logical_cpu_count(),
        cores,
        cpu_info,
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

fn host_logical_cpu_count() -> Option<u32> {
    let configured = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    (configured > 0)
        .then(|| u32::try_from(configured).ok())
        .flatten()
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
    let recorded_counters = driver.counters();
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
                let mut callstack = smallvec::smallvec![CallFrame::IP(sample.ip)];
                callstack.extend(
                    sample
                        .callstack
                        .into_iter()
                        .filter(|address| *address != sample.ip)
                        .map(CallFrame::IP),
                );
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
        counters: recorded_counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
    }))
}

fn publish_process_maps(dispatcher: Arc<EventDispatcher>, pid: i32) {
    #[cfg(target_os = "macos")]
    if let Ok(images) = proc_maps::mac_maps::get_dyld_info(pid as proc_maps::Pid)
        && !images.is_empty()
    {
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
            // Linux executable mappings normally begin at a non-zero ELF file
            // offset. Dropping it creates a second, overlapping module with a
            // bogus load bias alongside PERF_RECORD_MMAP. Which one framehop
            // sees first then depends on HashMap iteration order, making two
            // otherwise identical recordings unwind into unrelated functions.
            #[cfg(target_os = "linux")]
            offset: map.offset,
            #[cfg(not(target_os = "linux"))]
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

fn topdown(dispatcher: Arc<EventDispatcher>, command: &[String]) -> Result<ScenarioInfo> {
    let scenario = pmu::host_tma_scenario().context("TMA is not supported on this CPU")?;
    let process = Process::new(command, &[])?;
    // Validate the formula groups, but do not turn each one into an independent
    // sampling leader. Multiple cycle leaders multiply the interrupt rate and
    // severely perturb the workload (especially while capturing DWARF stacks).
    // The original TMA collector sampled the deduplicated event set once.
    get_tma_counter_groups(&scenario)?;
    let counters = get_pmu_counters(Scenario::TMA);

    // TMA uses the same sampling engine and attribution mode as Snapshot.
    // Only the counter set differs. The original TMA implementation worked
    // this way; switching selected scenarios to PEBS changed the meaning and
    // availability of samples without changing the metric formulas.
    let mut driver = pmu::SamplingDriverBuilder::new()
        .counters(&counters)
        .process(&process)
        .build()?;
    let recorded_counters = driver.counters();
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
        counters: recorded_counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
        groups: scenario.groups,
        precise_attribution: scenario.precise_attribution,
        metrics: scenario.metrics,
        constants: scenario.constants,
        ui: scenario.ui,
    }))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{VM_PROT_EXECUTE, mach_o_text_address, macos_segment_is_executable};

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
