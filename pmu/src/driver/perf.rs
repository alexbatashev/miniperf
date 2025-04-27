mod binding;
mod events;
mod mmap;

use hashbrown::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use events::process_counter;
use libc::{close, mmap, munmap, sysconf, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use mmap::{EventValue, ReadFormat, Records};
use perf_event_open_sys::bindings::{
    perf_event_attr, PERF_SAMPLE_CALLCHAIN, PERF_SAMPLE_CPU, PERF_SAMPLE_ID, PERF_SAMPLE_IP,
    PERF_SAMPLE_READ, PERF_SAMPLE_TID, PERF_SAMPLE_TIME,
};
use perf_event_open_sys::{self as sys, bindings::PERF_SAMPLE_IDENTIFIER};
use smallvec::SmallVec;

use crate::driver::{ProcAddr, Sample};
use crate::{Counter, Error, Record};

pub use events::list_supported_counters;

use super::{CounterResult, CounterValue, CountingDriver, SamplingCallback, SamplingDriver};

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
}

#[derive(Debug, Clone)]
struct NativeCounterHandle {
    pub kind: Counter,
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
        let mut attrs = get_native_counters(&counters, false)?;

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
}

impl CountingDriver for PerfCountingDriver {
    fn start(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::ENABLE(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::DISABLE(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }

        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error> {
        let res_enable = unsafe {
            sys::ioctls::RESET(
                self.native_handles.first().unwrap().fd,
                sys::bindings::PERF_IOC_FLAG_GROUP,
            )
        };

        if res_enable < 0 {
            return Err(Error::EnableFailed);
        }

        Ok(())
    }

    fn counters(&mut self) -> Result<CounterResult, std::io::Error> {
        let read_size = std::mem::size_of::<ReadFormat>() + (std::mem::size_of::<EventValue>());

        let mut buffer = vec![0_u8; read_size];
        let mut scaled_values =
            SmallVec::<[(Counter, CounterValue); 16]>::with_capacity(self.native_handles.len());

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
            let scaled_value = if header.time_running > 0 {
                (value.value as f64 * scaling_factor) as u64
            } else {
                value.value
            };
            scaled_values.push((
                handle.kind.clone(),
                CounterValue {
                    value: scaled_value,
                    scaling: scaling_factor,
                },
            ));
        }

        Ok(CounterResult {
            values: scaled_values,
        })
    }
}

unsafe impl Send for PerfSamplingDriver {}
unsafe impl Sync for PerfSamplingDriver {}

impl SamplingDriver for PerfSamplingDriver {
    fn start(&mut self, callback: Arc<dyn SamplingCallback>) -> Result<(), Error> {
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let mmaps = self.mmaps.clone();
        let native_handles = self.native_handles.clone();

        #[derive(Clone, Default)]
        struct LastSample {
            time_enabled: u64,
            time_running: u64,
            value: u64,
        }

        let handle = thread::spawn(move || {
            let mut last_samples_map = HashMap::<(usize, u32, u32, u32, u64), LastSample>::new();

            while running.load(Ordering::SeqCst) {
                for (idx, &mmap) in mmaps.iter().enumerate() {
                    let records = Records::from_ptr(mmap.ptr);

                    for record in records.into_iter() {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
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
                            } => {
                                let uid = uuid::Uuid::now_v7();

                                for value in values {
                                    let handle = native_handles
                                        .iter()
                                        .find(|handle| handle.id == value.id)
                                        .unwrap();
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
                                        time,
                                        time_enabled: time_enabled - last_sample.time_enabled,
                                        time_running: time_running - last_sample.time_running,
                                        counter: handle.kind.clone(),
                                        value: value.value - last_sample.value,
                                        callstack: callstack.clone(),
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
                return Err(Error::EnableFailed);
            }
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            handle.join().map_err(|_| Error::EnableFailed)?;
        }

        Ok(())
    }
}

impl PerfSamplingDriver {
    pub fn new(
        counters: &[Counter],
        sample_freq: u64,
        pid: Option<i32>,
        prefer_raw_events: bool,
    ) -> Result<PerfSamplingDriver, Error> {
        let mut attrs = get_native_counters(counters, prefer_raw_events)?;

        for attr in &mut attrs {
            attr.set_exclude_kernel(1);
            attr.set_exclude_user(0);
            attr.set_exclusive(0);
            attr.set_inherit(0);
            attr.set_enable_on_exec(1);

            attr.sample_freq = sample_freq;
            attr.set_freq(1);

            attr.sample_type = (PERF_SAMPLE_IP
                | PERF_SAMPLE_TID
                | PERF_SAMPLE_TIME
                | PERF_SAMPLE_ID
                | PERF_SAMPLE_CPU
                | PERF_SAMPLE_READ
                | PERF_SAMPLE_CALLCHAIN) as u64;

            attr.set_mmap(1);
        }

        let native_handles = binding::grouped(counters, &mut attrs, pid)?;

        let page_size = unsafe { sysconf(libc::_SC_PAGE_SIZE) } as usize;
        let mmap_pages = 512;

        let mmaps = native_handles
            .iter()
            .filter(|native_handle| native_handle.leader)
            .map(|handle| unsafe {
                let ptr = mmap(
                    std::ptr::null_mut(),
                    page_size * (mmap_pages + 1),
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    handle.fd,
                    0,
                ) as *mut u8;
                if ptr as *mut libc::c_void == MAP_FAILED {
                    let err = std::io::Error::last_os_error();
                    panic!(
                        "Failed to map {:?} : len = {} fd = {}",
                        err.raw_os_error(),
                        page_size * (mmap_pages + 1),
                        handle.fd
                    );
                }
                UnsafeMmap { ptr }
            })
            .collect();

        Ok(PerfSamplingDriver {
            native_handles,
            mmaps,
            page_size,
            mmap_pages,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        })
    }
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

fn get_native_counters(
    counters: &[Counter],
    prefer_raw_counters: bool,
) -> Result<Vec<perf_event_attr>, Error> {
    let attrs = counters
        .iter()
        .map(|cntr| {
            let mut attrs = perf_event_attr::default();

            attrs.size = std::mem::size_of::<perf_event_attr>() as u32;
            attrs.set_disabled(1);

            attrs.read_format = sys::bindings::PERF_FORMAT_GROUP as u64
                | sys::bindings::PERF_FORMAT_ID as u64
                | sys::bindings::PERF_FORMAT_TOTAL_TIME_ENABLED as u64
                | sys::bindings::PERF_FORMAT_TOTAL_TIME_RUNNING as u64;

            let cntr = process_counter(cntr, prefer_raw_counters);

            match cntr {
                Counter::Cycles => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_CPU_CYCLES as u64;
                }
                Counter::Instructions => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_INSTRUCTIONS as u64;
                }
                Counter::LLCMisses => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_CACHE_MISSES as u64;
                }
                Counter::LLCReferences => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_CACHE_REFERENCES as u64;
                }
                Counter::BranchInstructions => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_BRANCH_INSTRUCTIONS as u64;
                }
                Counter::BranchMisses => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_BRANCH_MISSES as u64;
                }
                Counter::StalledCyclesFrontend => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_FRONTEND as u64;
                }
                Counter::StalledCyclesBackend => {
                    attrs.type_ = sys::bindings::PERF_TYPE_HARDWARE;
                    attrs.config = sys::bindings::PERF_COUNT_HW_STALLED_CYCLES_BACKEND as u64;
                }
                Counter::CpuClock => {
                    attrs.type_ = sys::bindings::PERF_TYPE_SOFTWARE;
                    attrs.config = sys::bindings::PERF_COUNT_SW_CPU_CLOCK as u64;
                }
                Counter::ContextSwitches => {
                    attrs.type_ = sys::bindings::PERF_TYPE_SOFTWARE;
                    attrs.config = sys::bindings::PERF_COUNT_SW_CONTEXT_SWITCHES as u64;
                }
                Counter::CpuMigrations => {
                    attrs.type_ = sys::bindings::PERF_TYPE_SOFTWARE;
                    attrs.config = sys::bindings::PERF_COUNT_SW_CPU_MIGRATIONS as u64;
                }
                Counter::PageFaults => {
                    attrs.type_ = sys::bindings::PERF_TYPE_SOFTWARE;
                    attrs.config = sys::bindings::PERF_COUNT_SW_PAGE_FAULTS as u64;
                }
                Counter::Internal {
                    name: _,
                    desc: _,
                    code,
                } => {
                    attrs.type_ = sys::bindings::PERF_TYPE_RAW;
                    attrs.config = code;
                }
                _ => todo!(),
            }

            attrs
        })
        .collect::<Vec<_>>();

    Ok(attrs)
}
