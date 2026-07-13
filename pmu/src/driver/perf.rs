mod binding;
mod events;
mod mmap;

use hashbrown::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use events::process_counter;
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use events::resolve_counter_for_family;
use libc::{close, mmap, munmap, sysconf, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use mmap::{EventValue, ReadFormat, Records};
use perf_event_open_sys::bindings::{
    perf_event_attr, PERF_SAMPLE_BRANCH_CALL_STACK, PERF_SAMPLE_BRANCH_STACK,
    PERF_SAMPLE_BRANCH_USER, PERF_SAMPLE_CALLCHAIN, PERF_SAMPLE_CPU, PERF_SAMPLE_ID,
    PERF_SAMPLE_IP, PERF_SAMPLE_READ, PERF_SAMPLE_REGS_USER, PERF_SAMPLE_STACK_USER,
    PERF_SAMPLE_TID, PERF_SAMPLE_TIME,
};
use perf_event_open_sys::{self as sys, bindings::PERF_SAMPLE_IDENTIFIER};
use smallvec::SmallVec;

use crate::driver::{ProcAddr, Sample, UnwindMode};
use crate::{Counter, Error, Record};

pub use events::list_supported_counters;

use super::{
    CoreId, CounterEntry, CounterResult, CounterValue, CountingDriver, MeasurementQuality,
    SamplingCallback, SamplingDriver,
};

/// Counting driver is used for simple collection of system's performance counters values. On Linux,
/// counter multiplexing is supported.
pub struct PerfCountingDriver {
    native_handles: Vec<NativeCounterHandle>,
}

/// Sampling driver performs PMU event sampling. That is, every N cycles, the process is
/// interrupted and counters values are recorded for future post processing.
pub struct PerfSamplingDriver {
    native_handles: Vec<NativeCounterHandle>,
    mmaps: Vec<UnsafeMmap>,
    page_size: usize,
    mmap_pages: usize,
    running: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
    enable_on_start: bool,
    sample_regs_user: u64,
    sample_branch_stack: bool,
}

#[derive(Debug, Clone)]
struct NativeCounterHandle {
    pub kind: Counter,
    /// The core cluster this handle counts on, on a heterogeneous system.
    /// `None` for software counters and on homogeneous systems.
    pub core: Option<CoreId>,
    pub id: u64,
    pub fd: i32,
    pub leader: bool,
}

#[derive(Debug, Clone, Copy)]
struct UnsafeMmap {
    ptr: *mut u8,
}

unsafe impl Send for UnsafeMmap {}
unsafe impl Sync for UnsafeMmap {}

impl PerfCountingDriver {
    pub fn new(counters: Vec<Counter>, pid: Option<i32>) -> Result<Self, Error> {
        // On a heterogeneous (big.LITTLE) host we open every hardware counter on
        // each cluster's PMU so a migrating task is faithfully counted wherever
        // it runs. `host_core_pmus` returns more than one entry only in that
        // case; otherwise fall through to the single-PMU path below.
        let core_pmus = crate::cpu_family::host_core_pmus();
        if core_pmus.len() > 1 {
            #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
            return Self::new_per_core(counters, pid, &core_pmus);
        }

        let mut attrs = get_native_counters(&counters, true)?;

        for attr in &mut attrs {
            attr.set_exclude_kernel(1);
            attr.set_exclude_hv(1);
            attr.set_inherit(1);
            attr.set_exclusive(0);
            attr.sample_type = PERF_SAMPLE_IDENTIFIER as u64;
            if pid.is_some() {
                attr.set_enable_on_exec(1);
            }
        }

        let native_handles = binding::direct(&counters, &mut attrs, pid)?;

        Ok(PerfCountingDriver { native_handles })
    }

    /// Open each PMU counter once per core cluster (faithful per-core counting).
    /// Software counters, which are not PMU-specific, are opened a single time.
    /// A counter that a cluster's family does not implement is skipped there.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    fn new_per_core(
        counters: Vec<Counter>,
        pid: Option<i32>,
        core_pmus: &[crate::cpu_family::CorePmu],
    ) -> Result<Self, Error> {
        let apply_flags = |attr: &mut perf_event_attr| {
            attr.set_exclude_kernel(1);
            attr.set_exclude_hv(1);
            attr.set_inherit(1);
            attr.set_exclusive(0);
            attr.sample_type = PERF_SAMPLE_IDENTIFIER as u64;
            if pid.is_some() {
                attr.set_enable_on_exec(1);
            }
        };

        let open = |counter: &Counter, attr: &mut perf_event_attr| -> Result<(i32, u64), Error> {
            let fd = unsafe { sys::perf_event_open(attr, pid.unwrap_or(0), -1, -1, 0) };
            if fd < 0 {
                return Err(Error::perf_event_open(counter, None));
            }
            let mut id: u64 = 0;
            if unsafe { sys::ioctls::ID(fd, &mut id) } < 0 {
                let error = Error::perf_ioctl("ID", counter);
                unsafe { close(fd) };
                return Err(error);
            }
            Ok((fd, id))
        };

        let mut native_handles: Vec<NativeCounterHandle> = Vec::new();

        for cntr in &counters {
            if cntr.is_software() {
                let (type_, config) = counter_type_config(cntr)?;
                let mut attr = base_counter_attr();
                attr.type_ = type_;
                attr.config = config;
                apply_flags(&mut attr);

                let (fd, id) = open(cntr, &mut attr)?;
                native_handles.push(NativeCounterHandle {
                    kind: cntr.clone(),
                    core: None,
                    id,
                    fd,
                    leader: false,
                });
                continue;
            }

            for pmu in core_pmus {
                let Some(resolved) = resolve_counter_for_family(cntr, pmu.family_id, true) else {
                    continue; // this cluster's family does not implement it
                };

                let mut attr = build_pmu_attr(&resolved, pmu.pmu_type)?;
                apply_flags(&mut attr);

                let (fd, id) = open(cntr, &mut attr)?;
                native_handles.push(NativeCounterHandle {
                    kind: cntr.clone(),
                    core: Some(core_id_of(pmu)),
                    id,
                    fd,
                    leader: false,
                });
            }
        }

        if native_handles.is_empty() {
            return Err(Error::InvalidConfiguration(
                "no counters could be opened".to_owned(),
            ));
        }

        Ok(PerfCountingDriver { native_handles })
    }
}

/// Build a display-friendly [`CoreId`] for a core PMU, resolving the family's
/// human readable name where known.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn core_id_of(pmu: &crate::cpu_family::CorePmu) -> CoreId {
    let name = crate::cpu_family::find_cpu_family(pmu.family_id)
        .map(|f| f.name.clone())
        .unwrap_or_else(|| pmu.family_id.to_string());

    CoreId {
        family_id: pmu.family_id.to_string(),
        name,
        cpus: pmu.cpus.clone(),
    }
}

impl CountingDriver for PerfCountingDriver {
    fn start(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::ENABLE(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::perf_ioctl("ENABLE", &handle.kind));
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::DISABLE(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::perf_ioctl("DISABLE", &handle.kind));
            }
        }

        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error> {
        // Per-core counters are opened as independent events (not a group), so
        // reset each one individually rather than relying on the group flag.
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::RESET(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::perf_ioctl("RESET", &handle.kind));
            }
        }

        Ok(())
    }

    fn counters(&mut self) -> Result<CounterResult, std::io::Error> {
        let read_size = std::mem::size_of::<ReadFormat>() + (std::mem::size_of::<EventValue>());

        let mut buffer = vec![0_u8; read_size];
        let mut entries = SmallVec::<[CounterEntry; 16]>::with_capacity(self.native_handles.len());

        for handle in self.native_handles.iter() {
            let result = unsafe {
                libc::read(
                    handle.fd,
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    read_size,
                )
            };

            if result == -1 {
                return Err(std::io::Error::last_os_error());
            }

            let header = unsafe { &*(buffer.as_ptr() as *const ReadFormat) };

            let values = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr().add(std::mem::size_of::<ReadFormat>()) as *const EventValue,
                    header.nr as usize,
                )
            };

            // For now it is guaranteed there's exactly 1 event
            let value = &values[0];

            let scaling_factor = if header.time_running > 0 {
                (header.time_enabled as f64) / (header.time_running as f64)
            } else {
                1.0_f64
            };

            // For a per-core counter opened on a specific cluster's PMU, the
            // "enabled but not running" time is mostly time the task spent on
            // the *other* cluster, not counter multiplexing. Extrapolating over
            // it would massively inflate the value, so report the raw on-cluster
            // count instead — the true work done on that cluster. Summing the
            // raw per-cluster counts then yields a faithful total.
            //
            // Homogeneous and software counters keep the usual multiplexing
            // extrapolation, where enabled/running reflects real time-sharing.
            let reported_value = if handle.core.is_some() {
                value.value
            } else if header.time_running > 0 {
                (value.value as f64 * scaling_factor) as u64
            } else {
                value.value
            };

            entries.push(CounterEntry {
                core: handle.core.clone(),
                counter: handle.kind.clone(),
                value: CounterValue {
                    value: reported_value,
                    scaling: scaling_factor,
                    quality: if handle.core.is_none() && scaling_factor > 1.0 {
                        MeasurementQuality::Scaled
                    } else {
                        MeasurementQuality::Exact
                    },
                },
            });
        }

        Ok(CounterResult::from_entries(entries))
    }
}

unsafe impl Send for PerfSamplingDriver {}
unsafe impl Sync for PerfSamplingDriver {}

impl SamplingDriver for PerfSamplingDriver {
    fn counters(&self) -> Vec<Counter> {
        let mut counters = Vec::new();
        for handle in &self.native_handles {
            if !counters.contains(&handle.kind) {
                counters.push(handle.kind.clone());
            }
        }
        counters
    }

    fn start(&mut self, callback: Arc<dyn SamplingCallback>) -> Result<(), Error> {
        if self.enable_on_start {
            for handle in self.native_handles.iter().filter(|handle| handle.leader) {
                let result =
                    unsafe { sys::ioctls::ENABLE(handle.fd, sys::bindings::PERF_IOC_FLAG_GROUP) };
                if result < 0 {
                    return Err(Error::perf_ioctl("ENABLE group", &handle.kind));
                }
            }
        }
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let mmaps = self.mmaps.clone();
        let native_handles = self.native_handles.clone();
        let sample_regs_user = self.sample_regs_user;
        let sample_branch_stack = self.sample_branch_stack;

        #[derive(Clone, Default)]
        struct LastSample {
            time_enabled: u64,
            time_running: u64,
            value: u64,
        }

        let handle = thread::spawn(move || {
            let mut last_samples_map = HashMap::<(usize, u32, u32, u32, u64), LastSample>::new();

            loop {
                for (idx, &mmap) in mmaps.iter().enumerate() {
                    let records =
                        Records::from_ptr(mmap.ptr, sample_regs_user, sample_branch_stack);

                    for record in records.into_iter() {
                        match record {
                            mmap::MmapRecord::Sample {
                                ip,
                                pid,
                                tid,
                                cpu,
                                time,
                                time_enabled,
                                time_running,
                                values,
                                callstack,
                                user_regs,
                                user_stack,
                            } => {
                                let uid = uuid::Uuid::now_v7();
                                let mut user_regs = user_regs;
                                let mut user_stack = Some(user_stack);

                                for value in values {
                                    let Some(handle) =
                                        native_handles.iter().find(|handle| handle.id == value.id)
                                    else {
                                        continue;
                                    };
                                    let last_sample = last_samples_map
                                        .get(&(idx, cpu, pid, tid, value.id))
                                        .cloned()
                                        .unwrap_or_default();

                                    let sample = Record::Sample(Sample {
                                        event_id: uid.as_u128(),
                                        ip,
                                        pid,
                                        tid,
                                        cpu,
                                        core: handle.core.as_ref().map(|c| c.family_id.clone()),
                                        time,
                                        time_enabled: time_enabled - last_sample.time_enabled,
                                        time_running: time_running - last_sample.time_running,
                                        counter: handle.kind.clone(),
                                        value: value.value - last_sample.value,
                                        callstack: callstack.clone(),
                                        // Every grouped counter shares this correlation id and
                                        // call stack. Carry the large raw state once; postprocess
                                        // reuses its result for the sibling counter events.
                                        user_regs: user_regs.take(),
                                        user_stack: user_stack.take().unwrap_or_default(),
                                    });

                                    last_samples_map.insert(
                                        (idx, cpu, pid, tid, value.id),
                                        LastSample {
                                            time_enabled,
                                            time_running,
                                            value: value.value,
                                        },
                                    );

                                    callback.call(sample);
                                }
                            }
                            mmap::MmapRecord::Address {
                                pid,
                                start,
                                len,
                                offset,
                                filename,
                            } => {
                                callback.call(Record::ProcAddr(ProcAddr {
                                    pid,
                                    addr: start,
                                    len,
                                    pgoff: offset,
                                    filename,
                                }));
                            }
                            mmap::MmapRecord::Unknown => {}
                        }
                    }
                }

                if !running.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_micros(100));
            }
        });

        self.thread_handle = Some(handle);

        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            if !handle.leader {
                continue;
            }

            let res_enable =
                unsafe { sys::ioctls::DISABLE(handle.fd, sys::bindings::PERF_IOC_FLAG_GROUP) };

            if res_enable < 0 {
                return Err(Error::perf_ioctl("DISABLE", &handle.kind));
            }
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            handle.join().map_err(|_| Error::WorkerPanicked)?;
        }

        Ok(())
    }
}

/// Apply the sampling-specific attribute flags shared by every counter.
fn apply_sampling_flags(
    attr: &mut perf_event_attr,
    sample_freq: u64,
    unwind_mode: UnwindMode,
    stack_dump_size: u32,
    enable_on_exec: bool,
    precise_ip: bool,
) {
    attr.set_exclude_kernel(1);
    attr.set_exclude_user(0);
    attr.set_exclusive(0);
    attr.set_inherit(0);
    attr.set_enable_on_exec(enable_on_exec.into());
    if precise_ip {
        attr.set_precise_ip(2);
    }

    attr.sample_freq = sample_freq;
    attr.set_freq(1);

    let mut sample_type = (PERF_SAMPLE_IP
        | PERF_SAMPLE_TID
        | PERF_SAMPLE_TIME
        | PERF_SAMPLE_ID
        | PERF_SAMPLE_CPU
        | PERF_SAMPLE_READ
        | PERF_SAMPLE_CALLCHAIN) as u64;

    if unwind_mode == UnwindMode::Dwarf {
        let regs = dwarf_register_mask();
        if regs != 0 {
            sample_type |= (PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER) as u64;
            attr.sample_regs_user = regs;
            attr.sample_stack_user = stack_dump_size;
        }
    }
    if unwind_mode == UnwindMode::Lbr && cfg!(target_arch = "x86_64") {
        sample_type |= PERF_SAMPLE_BRANCH_STACK as u64;
        attr.branch_sample_type = (PERF_SAMPLE_BRANCH_CALL_STACK | PERF_SAMPLE_BRANCH_USER) as u64;
    }
    attr.sample_type = sample_type;

    attr.set_mmap(1);
}

impl PerfSamplingDriver {
    pub fn new(
        counters: &[Counter],
        sample_freq: u64,
        pid: Option<i32>,
        prefer_raw_events: bool,
        unwind_mode: UnwindMode,
        stack_dump_size: u32,
        precise_ip: bool,
    ) -> Result<PerfSamplingDriver, Error> {
        // On a heterogeneous (big.LITTLE) host, open a sampling group on each
        // cluster's PMU so the profile captures execution wherever the task
        // runs, not just on one cluster.
        let core_pmus = crate::cpu_family::host_core_pmus();
        if core_pmus.len() > 1 {
            #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
            return Self::new_per_core(
                counters,
                sample_freq,
                pid,
                &core_pmus,
                unwind_mode,
                stack_dump_size,
                precise_ip,
            );
        }

        let mut attrs = get_native_counters(counters, prefer_raw_events)?;

        for attr in &mut attrs {
            apply_sampling_flags(
                attr,
                sample_freq,
                unwind_mode,
                stack_dump_size,
                pid.is_some(),
                precise_ip,
            );
        }

        let native_handles = if pid.is_none() {
            binding::grouped_all(counters, &mut attrs, pid)?
        } else if counters.contains(&Counter::Cycles) {
            binding::grouped(counters, &mut attrs, pid)?
        } else {
            binding::grouped_software(counters, &mut attrs, pid)?
        };

        Self::from_handles(
            native_handles,
            dwarf_mask_for_mode(unwind_mode),
            unwind_mode == UnwindMode::Lbr,
            pid.is_none(),
        )
    }

    /// Faithful per-core sampling: open a sampling group on every cluster's PMU
    /// (each with that cluster's event codes), so no cluster is invisible in the
    /// profile. Each handle is tagged with the cluster it samples so downstream
    /// consumers can attribute samples per core.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    fn new_per_core(
        counters: &[Counter],
        sample_freq: u64,
        pid: Option<i32>,
        core_pmus: &[crate::cpu_family::CorePmu],
        unwind_mode: UnwindMode,
        stack_dump_size: u32,
        precise_ip: bool,
    ) -> Result<PerfSamplingDriver, Error> {
        let mut native_handles: Vec<NativeCounterHandle> = Vec::new();

        for pmu in core_pmus {
            let core = core_id_of(pmu);

            let mut attrs: Vec<perf_event_attr> = counters
                .iter()
                .map(|cntr| {
                    let resolved = resolve_counter_for_family(cntr, pmu.family_id, true)
                        .unwrap_or_else(|| cntr.clone());
                    let mut attr = build_pmu_attr(&resolved, pmu.pmu_type)?;
                    apply_sampling_flags(
                        &mut attr,
                        sample_freq,
                        unwind_mode,
                        stack_dump_size,
                        pid.is_some(),
                        precise_ip,
                    );
                    Ok(attr)
                })
                .collect::<Result<Vec<_>, Error>>()?;

            let mut handles = if pid.is_none() {
                binding::grouped_all(counters, &mut attrs, pid)?
            } else {
                binding::grouped(counters, &mut attrs, pid)?
            };
            for handle in &mut handles {
                handle.core = Some(core.clone());
            }

            native_handles.extend(handles);
        }

        Self::from_handles(
            native_handles,
            dwarf_mask_for_mode(unwind_mode),
            unwind_mode == UnwindMode::Lbr,
            pid.is_none(),
        )
    }

    /// Map every group-leader handle's ring buffer and assemble the driver.
    fn from_handles(
        native_handles: Vec<NativeCounterHandle>,
        sample_regs_user: u64,
        sample_branch_stack: bool,
        enable_on_start: bool,
    ) -> Result<PerfSamplingDriver, Error> {
        let page_size = unsafe { sysconf(libc::_SC_PAGE_SIZE) } as usize;
        let mmap_pages = 512;

        let length = page_size * (mmap_pages + 1);
        let mut mmaps: Vec<UnsafeMmap> = Vec::new();
        for handle in native_handles.iter().filter(|handle| handle.leader) {
            let ptr = unsafe {
                let ptr = mmap(
                    std::ptr::null_mut(),
                    length,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    handle.fd,
                    0,
                ) as *mut u8;
                if ptr as *mut libc::c_void == MAP_FAILED {
                    let source = std::io::Error::last_os_error();
                    for mmap in &mmaps {
                        munmap(mmap.ptr.cast(), length);
                    }
                    for native_handle in &native_handles {
                        close(native_handle.fd);
                    }
                    return Err(Error::PerfMmap {
                        counter: handle.kind.name().to_owned(),
                        length,
                        source,
                    });
                }
                ptr
            };
            mmaps.push(UnsafeMmap { ptr });
        }

        Ok(PerfSamplingDriver {
            native_handles,
            mmaps,
            page_size,
            mmap_pages,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            sample_regs_user,
            sample_branch_stack,
            enable_on_start,
        })
    }
}

fn dwarf_mask_for_mode(mode: UnwindMode) -> u64 {
    if mode == UnwindMode::Dwarf {
        dwarf_register_mask()
    } else {
        0
    }
}

#[cfg(target_arch = "x86_64")]
fn dwarf_register_mask() -> u64 {
    // Linux's PERF_REGS_MASK: segment registers DS/ES/FS/GS (bits 12..15)
    // cannot be requested through PERF_SAMPLE_REGS_USER on x86-64.
    ((1_u64 << sys::bindings::PERF_REG_X86_64_MAX) - 1) & !(0xf_u64 << 12)
}

#[cfg(target_arch = "aarch64")]
fn dwarf_register_mask() -> u64 {
    // x0..x29, link register, SP, and PC.
    (1_u64 << sys::bindings::PERF_REG_ARM64_MAX) - 1
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn dwarf_register_mask() -> u64 {
    0
}

impl Drop for PerfSamplingDriver {
    fn drop(&mut self) {
        for &mmap in &self.mmaps {
            unsafe {
                munmap(
                    mmap.ptr as *mut std::ffi::c_void,
                    self.page_size * (self.mmap_pages + 1),
                );
            }
        }
        for handle in &self.native_handles {
            unsafe { close(handle.fd) };
        }
    }
}

/// Base `perf_event_attr` shared by every counter: disabled at creation and
/// configured for grouped reads with scaling info.
fn base_counter_attr() -> perf_event_attr {
    let mut attrs = perf_event_attr::default();

    attrs.size = std::mem::size_of::<perf_event_attr>() as u32;
    attrs.set_disabled(1);

    attrs.read_format = sys::bindings::PERF_FORMAT_GROUP as u64
        | sys::bindings::PERF_FORMAT_ID as u64
        | sys::bindings::PERF_FORMAT_TOTAL_TIME_ENABLED as u64
        | sys::bindings::PERF_FORMAT_TOTAL_TIME_RUNNING as u64;

    attrs
}

/// Map a resolved counter to its `(type_, config)` pair using the legacy
/// generic encodings (`PERF_TYPE_HARDWARE`/`SOFTWARE`/`RAW`).
fn counter_type_config(cntr: &Counter) -> Result<(u32, u64), Error> {
    Ok(match cntr {
        Counter::Cycles => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_CPU_CYCLES as u64,
        ),
        Counter::Instructions => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_INSTRUCTIONS as u64,
        ),
        Counter::LLCMisses => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_CACHE_MISSES as u64,
        ),
        Counter::LLCReferences => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_CACHE_REFERENCES as u64,
        ),
        Counter::BranchInstructions => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_BRANCH_INSTRUCTIONS as u64,
        ),
        Counter::BranchMisses => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_BRANCH_MISSES as u64,
        ),
        Counter::StalledCyclesFrontend => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_FRONTEND as u64,
        ),
        Counter::StalledCyclesBackend => (
            sys::bindings::PERF_TYPE_HARDWARE,
            sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_BACKEND as u64,
        ),
        Counter::CpuClock => (
            sys::bindings::PERF_TYPE_SOFTWARE,
            sys::bindings::PERF_COUNT_SW_CPU_CLOCK as u64,
        ),
        Counter::ContextSwitches => (
            sys::bindings::PERF_TYPE_SOFTWARE,
            sys::bindings::PERF_COUNT_SW_CONTEXT_SWITCHES as u64,
        ),
        Counter::CpuMigrations => (
            sys::bindings::PERF_TYPE_SOFTWARE,
            sys::bindings::PERF_COUNT_SW_CPU_MIGRATIONS as u64,
        ),
        Counter::PageFaults => (
            sys::bindings::PERF_TYPE_SOFTWARE,
            sys::bindings::PERF_COUNT_SW_PAGE_FAULTS as u64,
        ),
        Counter::Internal { code, .. } => (sys::bindings::PERF_TYPE_RAW, *code),
        Counter::Custom(name) => {
            return Err(Error::InvalidConfiguration(format!(
                "custom counter '{name}' was not resolved"
            )))
        }
    })
}

fn get_native_counters(
    counters: &[Counter],
    prefer_raw_counters: bool,
) -> Result<Vec<perf_event_attr>, Error> {
    let attrs = counters
        .iter()
        .map(|cntr| {
            let mut attrs = base_counter_attr();

            let cntr = process_counter(cntr, prefer_raw_counters)?;
            let (type_, config) = counter_type_config(&cntr)?;
            attrs.type_ = type_;
            attrs.config = config;

            // On heterogeneous (big.LITTLE) AArch64 systems the legacy
            // PERF_TYPE_RAW / PERF_TYPE_HARDWARE encodings bind to a single
            // cluster's PMU, so events silently fail to count when the task
            // runs on the other cluster. Route hardware and raw events to the
            // dynamic PMU type that backs the detected host CPU family instead.
            // (The full per-core counting path uses `build_pmu_attr` to open on
            // every cluster; this keeps the single-PMU/sampling path correct.)
            #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
            if let Some(pmu_type) = crate::cpu_family::host_pmu_type() {
                if attrs.type_ == sys::bindings::PERF_TYPE_RAW {
                    attrs.type_ = pmu_type;
                } else if attrs.type_ == sys::bindings::PERF_TYPE_HARDWARE {
                    if let Some(code) = aarch64_hw_event_code(attrs.config) {
                        attrs.type_ = pmu_type;
                        attrs.config = code;
                    }
                }
            }

            Ok(attrs)
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(attrs)
}

/// Build a `perf_event_attr` for a resolved counter bound to a *specific* core
/// PMU (`pmu_type`). Hardware and raw events are routed to that PMU so they
/// count only while the task runs on that cluster; software events are left on
/// the generic software PMU.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn build_pmu_attr(resolved: &Counter, pmu_type: u32) -> Result<perf_event_attr, Error> {
    let mut attrs = base_counter_attr();
    let (type_, config) = counter_type_config(resolved)?;

    if type_ == sys::bindings::PERF_TYPE_RAW {
        attrs.type_ = pmu_type;
        attrs.config = config;
    } else if type_ == sys::bindings::PERF_TYPE_HARDWARE {
        if let Some(code) = aarch64_hw_event_code(config) {
            attrs.type_ = pmu_type;
            attrs.config = code;
        } else {
            attrs.type_ = type_;
            attrs.config = config;
        }
    } else {
        attrs.type_ = type_;
        attrs.config = config;
    }

    Ok(attrs)
}

/// Map a generic `PERF_COUNT_HW_*` config value to the equivalent AArch64
/// architectural raw event code, so hardware counters can be opened against a
/// specific core PMU on heterogeneous systems.
///
/// In practice the counting and sampling drivers request raw counters, so
/// generic hardware events are already remapped via the platform aliases before
/// they reach here; this is a defensive fallback for the non-raw path.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn aarch64_hw_event_code(config: u64) -> Option<u64> {
    let code = match config as u32 {
        sys::bindings::PERF_COUNT_HW_CPU_CYCLES => 0x11, // CPU_CYCLES
        sys::bindings::PERF_COUNT_HW_INSTRUCTIONS => 0x08, // INST_RETIRED
        sys::bindings::PERF_COUNT_HW_CACHE_REFERENCES => 0x36, // LL_CACHE_RD
        sys::bindings::PERF_COUNT_HW_CACHE_MISSES => 0x37, // LL_CACHE_MISS_RD
        sys::bindings::PERF_COUNT_HW_BRANCH_INSTRUCTIONS => 0x21, // BR_RETIRED
        sys::bindings::PERF_COUNT_HW_BRANCH_MISSES => 0x22, // BR_MIS_PRED_RETIRED
        sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_FRONTEND => 0x23, // STALL_FRONTEND
        sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_BACKEND => 0x24, // STALL_BACKEND
        _ => return None,
    };
    Some(code)
}
