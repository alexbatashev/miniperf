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
    duration: Option<std::time::Duration>,
) -> Result<()> {
    println!("Record profile with {scenario:?} scenario");

    let cpu_info = if scenario == Scenario::Mem
        && roofline::uses_native_performance(&roofline_options, &command)?
    {
        println!("Calibrating sustainable host memory bandwidth...");
        let calibration = roofline::calibrate_memory_host()?;
        println!(
            "Host memory ceiling: {:.2} GB/s ({} Rayon threads)",
            calibration.gbytes_per_second, calibration.threads
        );
        mperf_data::CpuInfo {
            memory_calibration: Some(Box::new(calibration)),
            roofline_calibration: None,
        }
    } else if scenario == Scenario::Roofline
        && roofline::uses_native_performance(&roofline_options, &command)?
    {
        println!("Calibrating host Roofline ceilings...");
        let calibration = roofline::calibrate_host()?;
        println!(
            "Host ceilings: {:.2} GFLOP/s FP64, {:.2} GB/s memory ({} Rayon threads)",
            calibration.fp64_gflops, calibration.memory_gbytes_per_second, calibration.threads
        );
        mperf_data::CpuInfo {
            memory_calibration: Some(Box::new((&calibration).into())),
            roofline_calibration: Some(Box::new(calibration)),
        }
    } else {
        if matches!(scenario, Scenario::Roofline | Scenario::Mem) {
            println!(
                "Skipping host Roofline calibration: the selected method does not measure native performance"
            );
        }
        mperf_data::CpuInfo::default()
    };

    let (dispatcher, join_handle) = EventDispatcher::new(output_directory);

    let info = match scenario {
        Scenario::Snapshot => snapshot(
            dispatcher.clone(),
            pid,
            &command,
            output_directory,
            duration,
        )?,
        Scenario::Mem => {
            if pid.is_some() {
                anyhow::bail!("record mem requires a command and does not support --pid");
            }
            roofline::record_memory(
                &roofline_options,
                dispatcher.clone(),
                &command,
                output_directory,
            )
            .await?
        }
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
        sampling_frequency_hz: Some(if scenario == Scenario::Snapshot {
            99
        } else {
            pmu::DEFAULT_SAMPLE_FREQUENCY_HZ
        }),
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
    output_directory: &Path,
    duration: Option<std::time::Duration>,
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

    let recorded_pid = pid.unwrap_or_else(|| process.as_ref().unwrap().pid() as u32) as i32;
    #[cfg(target_os = "linux")]
    let target_pids = if let Some(pid) = pid {
        let tree = crate::snapshot_resources::process_tree(pid);
        if tree.is_empty() {
            vec![pid]
        } else {
            tree.into_iter().map(|member| member.pid).collect()
        }
    } else {
        vec![recorded_pid as u32]
    };
    #[cfg(not(target_os = "linux"))]
    let target_pids = vec![recorded_pid as u32];
    let mut drivers = Vec::with_capacity(target_pids.len());
    for target_pid in target_pids {
        let builder = pmu::SamplingDriverBuilder::new().counters(&counters);
        #[cfg(target_os = "linux")]
        let mut builder = builder.sample_freq(99).stack_dump_size(2 * 1024);
        #[cfg(not(target_os = "linux"))]
        let mut builder = builder;
        if let Some(process) = &process {
            builder = builder.process(process);
        } else {
            builder = builder.pid(target_pid as i32);
        }
        drivers.push(builder.build()?);
    }
    let recorded_counters = drivers
        .first()
        .map(|driver| driver.counters())
        .unwrap_or_default();
    #[cfg(target_os = "linux")]
    let cgroup_scope =
        crate::snapshot_resources::CgroupScope::create(recorded_pid as u32, pid.is_none());
    #[cfg(target_os = "linux")]
    let resource_monitor = crate::snapshot_resources::ResourceMonitor::start(
        recorded_pid as u32,
        pid.is_none(),
        cgroup_scope.path(),
        output_directory,
    )?;
    #[cfg(target_os = "linux")]
    let bpf_monitor =
        crate::snapshot_resources::BpfMonitor::start(recorded_pid as u32, output_directory);
    // On macOS Process::new returns an already-exec'd, suspended child, so its
    // dyld mappings are available before the first instruction is profiled.
    // Attached processes are already live on every platform.
    if cfg!(target_os = "macos") || pid.is_some() {
        publish_process_maps(dispatcher.clone(), recorded_pid);
    }

    for driver in &mut drivers {
        driver.start(snapshot_callback(dispatcher.clone()))?;
    }
    #[cfg(target_os = "linux")]
    let stop_reason = {
        let started = std::time::Instant::now();
        let mut known_pids = std::collections::HashSet::new();
        let mut root_waited = false;
        let stop_reason;
        if let Some(process) = &process {
            process.cont();
            loop {
                let tree = crate::snapshot_resources::process_tree(recorded_pid as u32);
                for member in &tree {
                    if known_pids.insert((member.pid, member.start_ticks)) {
                        publish_process_maps(dispatcher.clone(), member.pid as i32);
                    }
                }
                if !root_waited
                    && tree
                        .iter()
                        .find(|member| member.pid == recorded_pid as u32)
                        .is_none_or(|member| member.state == b'Z')
                {
                    process.wait()?;
                    root_waited = true;
                }
                let live = tree.iter().any(|member| member.state != b'Z');
                if root_waited && !live {
                    stop_reason = "tree_exit".to_string();
                    break;
                }
                if duration.is_some_and(|limit| started.elapsed() >= limit) {
                    // A launched command belongs to this recording. Terminate every
                    // currently observed member so a bounded snapshot cannot leave
                    // an unexpected orphan workload behind.
                    for member in tree.iter().filter(|member| member.state != b'Z') {
                        unsafe { libc::kill(member.pid as i32, libc::SIGTERM) };
                    }
                    if !root_waited {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(2);
                        while std::time::Instant::now() < deadline {
                            if crate::snapshot_resources::process_tree(recorded_pid as u32)
                                .iter()
                                .find(|member| member.pid == recorded_pid as u32)
                                .is_none_or(|member| member.state == b'Z')
                            {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        for member in crate::snapshot_resources::process_tree(recorded_pid as u32)
                            .iter()
                            .filter(|member| member.state != b'Z')
                        {
                            unsafe { libc::kill(member.pid as i32, libc::SIGKILL) };
                        }
                        process.wait()?;
                    }
                    stop_reason = "duration".to_string();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        } else if let Some(pid) = pid {
            loop {
                let tree = crate::snapshot_resources::process_tree(pid);
                for member in &tree {
                    if known_pids.insert((member.pid, member.start_ticks)) {
                        publish_process_maps(dispatcher.clone(), member.pid as i32);
                    }
                }
                if !tree.iter().any(|member| member.state != b'Z') {
                    stop_reason = "tree_exit".to_string();
                    break;
                }
                if duration.is_some_and(|limit| started.elapsed() >= limit) {
                    stop_reason = "duration".to_string();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        } else {
            unreachable!("snapshot requires a command or PID")
        }
        stop_reason
    };
    #[cfg(not(target_os = "linux"))]
    let stop_reason = if let Some(process) = &process {
        process.cont();
        process.wait()?;
        "root_exit".to_string()
    } else if let Some(pid) = pid {
        let started = std::time::Instant::now();
        while unsafe { libc::kill(pid as i32, 0) } == 0
            && duration.is_none_or(|limit| started.elapsed() < limit)
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if duration.is_some_and(|limit| started.elapsed() >= limit) {
            "duration".to_string()
        } else {
            "root_exit".to_string()
        }
    } else {
        unreachable!("snapshot requires a command or PID")
    };
    for driver in &mut drivers {
        driver.stop()?;
    }
    #[cfg(target_os = "linux")]
    let mut collectors = resource_monitor.stop();
    #[cfg(target_os = "linux")]
    collectors.push(bpf_monitor.stop());
    #[cfg(target_os = "linux")]
    collectors.push(cgroup_scope.status());
    #[cfg(not(target_os = "linux"))]
    let collectors: Vec<mperf_data::SnapshotCollectorStatus> = Vec::new();

    let warnings = collectors
        .iter()
        .filter(|collector| collector.status != "available")
        .map(|collector| format!("{}: {}", collector.name, collector.message))
        .collect();

    Ok(ScenarioInfo::Snapshot(mperf_data::SnapshotInfo {
        pid: recorded_pid,
        counters: recorded_counters
            .iter()
            .map(|counter| (counter_to_event_ty(counter), counter.name().to_string()))
            .collect(),
        scope: if cfg!(target_os = "linux") {
            if pid.is_some() {
                "attached_tree_best_effort"
            } else {
                "launched_tree_inherited"
            }
        } else {
            "legacy_root_only"
        }
        .to_string(),
        interval_ms: if cfg!(target_os = "linux") { 1_000 } else { 0 },
        stop_reason,
        collectors,
        warnings,
    }))
}

fn snapshot_callback(dispatcher: Arc<EventDispatcher>) -> Arc<dyn pmu::SamplingCallback> {
    Arc::new(move |record| match record {
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
                dispatcher.string_id(name)
            } else {
                0
            };
            dispatcher.publish_event_sync(Event {
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
            });
        }
        Record::ProcAddr(addr) => dispatcher.publish_proc_map_sync(ProcMapEntry {
            filename: addr.filename,
            address: addr.addr as usize,
            size: addr.len as usize,
            offset: addr.pgoff as usize,
            pid: addr.pid,
        }),
    })
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
