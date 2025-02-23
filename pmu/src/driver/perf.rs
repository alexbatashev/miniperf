mod events;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use events::process_counter;
use libc::{close, mmap, munmap, sysconf, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use perf_event_open_sys::bindings::{
    perf_event_attr, perf_event_header, perf_event_mmap_page, PERF_RECORD_SAMPLE,
    PERF_SAMPLE_CALLCHAIN, PERF_SAMPLE_CPU, PERF_SAMPLE_ID, PERF_SAMPLE_IP, PERF_SAMPLE_READ,
    PERF_SAMPLE_TID, PERF_SAMPLE_TIME,
};
use perf_event_open_sys::{self as sys, bindings::PERF_SAMPLE_IDENTIFIER};
use smallvec::{SmallVec, ToSmallVec};

use crate::{Counter, Error, Process};

pub use events::list_supported_counters;

/// Counting driver is used for simple collection of system's performance counters values. On Linux,
/// counter multiplexing is supported.
pub struct CountingDriver {
    native_handles: Vec<NativeCounterHandle>,
}

/// Sampling driver performs PMU event sampling. That is, every N cycles, the process is
/// interrupted and counters values are recorded for future post processing.
pub struct SamplingDriver {
    native_handles: Vec<NativeCounterHandle>,
    mmaps: Vec<UnsafeMmap>,
    page_size: usize,
    mmap_pages: usize,
    running: Arc<AtomicBool>,
}

pub struct SamplingDriverBuilder {
    counters: Vec<Counter>,
    sample_freq: u64,
    pid: Option<i32>,
    prefer_raw_events: bool,
}

#[derive(Debug, Clone)]
pub struct CounterValue {
    pub value: u64,
    pub scaling: f64,
}

/// A structure that represents a single sample
#[derive(Debug)]
pub struct Sample {
    /// Unique ID shared by all samples of the event
    pub event_id: u64,
    /// Instruction pointer
    pub ip: u64,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
    /// Timestamp
    pub time: u64,
    pub time_enabled: u64,
    pub time_running: u64,
    pub counter: Counter,
    pub value: u64,
    pub callstack: SmallVec<[u64; 32]>,
}

#[derive(Debug, Clone)]
pub struct CounterResult {
    values: Vec<(Counter, CounterValue)>,
}

#[derive(Debug, Clone)]
struct NativeCounterHandle {
    pub kind: Counter,
    pub id: u64,
    pub fd: i32,
}

#[derive(Debug, Clone, Copy)]
struct UnsafeMmap {
    ptr: *mut u8,
}

unsafe impl Send for UnsafeMmap {}
unsafe impl Sync for UnsafeMmap {}

#[repr(C)]
struct ReadFormat {
    nr: u64,
    time_enabled: u64,
    time_running: u64,
    values: [EventValue; 0],
}

/// Internal structure for reading from mmap ring buffer
#[repr(C)]
struct SampleFormat {
    header: perf_event_header,
    ip: u64,
    pid: u32,
    tid: u32,
    time: u64,
    id: u64,
    cpu: u32,
    _res: u32, // Reserved unused value
    read: ReadFormat,
}

#[repr(C)]
struct EventValue {
    value: u64,
    id: u64,
}

impl CountingDriver {
    pub fn new(counters: &[Counter], process: Option<&Process>) -> Result<Self, Error> {
        let mut attrs = get_native_counters(counters, false)?;

        for attr in &mut attrs {
            attr.set_exclude_kernel(1);
            attr.set_exclude_hv(1);
            attr.set_inherit(1);
            attr.set_exclusive(0);
            attr.sample_type = PERF_SAMPLE_IDENTIFIER as u64;
            if process.is_some() {
                attr.set_enable_on_exec(1);
            }
        }

        let pid = process.map(|p| p.pid());

        let native_handles = bind_counters(counters, &mut attrs, pid)?;

        Ok(CountingDriver { native_handles })
    }

    pub fn start(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe {
                sys::ioctls::ENABLE(
                    handle.fd,
                    0, // TODO support groups
                      // sys::bindings::PERF_IOC_FLAG_GROUP,
                )
            };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        for handle in &self.native_handles {
            let res_enable = unsafe { sys::ioctls::DISABLE(handle.fd, 0) };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }

        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), Error> {
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

    pub fn counters(&mut self) -> Result<CounterResult, std::io::Error> {
        let read_size = std::mem::size_of::<ReadFormat>() + (std::mem::size_of::<EventValue>());

        let mut buffer = vec![0_u8; read_size];
        let mut scaled_values = Vec::with_capacity(self.native_handles.len());

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

            let values =
                unsafe { std::slice::from_raw_parts(header.values.as_ptr(), header.nr as usize) };

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

unsafe impl Send for SamplingDriver {}
unsafe impl Sync for SamplingDriver {}

impl SamplingDriver {
    pub fn builder() -> SamplingDriverBuilder {
        SamplingDriverBuilder {
            counters: vec![],
            sample_freq: 1000,
            pid: None,
            prefer_raw_events: false,
        }
    }

    pub fn start<F>(&self, mut callback: F) -> Result<(), Error>
    where
        F: FnMut(Sample) + Send + 'static,
    {
        for handle in &self.native_handles {
            let res_enable = unsafe {
                sys::ioctls::ENABLE(
                    handle.fd,
                    0, // TODO support groups
                      // sys::bindings::PERF_IOC_FLAG_GROUP,
                )
            };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }

        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let mmaps = self.mmaps.clone();
        let page_size = self.page_size;
        let mmap_pages = self.mmap_pages;
        let native_handles = self.native_handles.clone();

        thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                for &mmap in &mmaps {
                    unsafe {
                        let data = mmap.ptr as *mut perf_event_mmap_page;
                        let data_head = (*data).data_head;
                        let data_tail = (*data).data_tail;

                        if data_head == data_tail {
                            continue;
                        }

                        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);

                        let base = mmap.ptr.add(page_size);
                        let buffer_size = page_size * mmap_pages;
                        let mut offset = data_tail as usize;

                        while offset < data_head as usize {
                            let offset_in_buffer = offset % buffer_size;

                            if offset_in_buffer + std::mem::size_of::<perf_event_header>()
                                > buffer_size
                            {
                                break;
                            }

                            let header = &*(base.add(offset_in_buffer) as *const perf_event_header);

                            if header.size as usize > buffer_size
                                || offset_in_buffer + header.size as usize > buffer_size
                            {
                                break;
                            }

                            if header.type_ == PERF_RECORD_SAMPLE {
                                let mut current_ptr =
                                    base.add(offset % (page_size * mmap_pages)) as *const u8;

                                // Read the fixed-size portion of the sample
                                let (next_ptr, format) = SampleFormat::read_from_ptr(current_ptr);
                                current_ptr = next_ptr;

                                // Read the variable-length values array
                                let (next_ptr, values) =
                                    SampleFormat::read_values(current_ptr, format.read.nr);
                                current_ptr = next_ptr;

                                // Read the callchain
                                let (_next_ptr, callstack) =
                                    SampleFormat::read_callchain(current_ptr);

                                let value = &values[0];

                                let handle = native_handles
                                    .iter()
                                    .find(|handle| handle.id == value.id)
                                    .unwrap();

                                let sample = Sample {
                                    event_id: format.id,
                                    ip: format.ip,
                                    pid: format.pid,
                                    tid: format.tid,
                                    time: format.time,
                                    time_enabled: format.read.time_enabled,
                                    time_running: format.read.time_running,
                                    counter: handle.kind.clone(),
                                    value: value.value,
                                    callstack: callstack[1..].to_smallvec(),
                                };

                                callback(sample);
                            }

                            offset += header.size as usize;
                        }

                        // Update data_tail
                        (*data).data_tail = data_head;
                    }
                }

                thread::sleep(Duration::from_micros(100));
            }
        });

        Ok(())
    }

    pub fn stop(&self) -> Result<(), Error> {
        self.running.store(false, Ordering::SeqCst);

        for handle in &self.native_handles {
            let res_enable = unsafe {
                sys::ioctls::DISABLE(
                    handle.fd,
                    0, // TODO support groups
                      // sys::bindings::PERF_IOC_FLAG_GROUP,
                )
            };

            if res_enable < 0 {
                return Err(Error::EnableFailed);
            }
        }
        Ok(())
    }
}

impl SamplingDriverBuilder {
    pub fn counters(self, counters: &[Counter]) -> Self {
        Self {
            counters: counters.to_vec(),
            sample_freq: self.sample_freq,
            pid: self.pid,
            prefer_raw_events: self.prefer_raw_events,
        }
    }

    pub fn process(self, process: &Process) -> Self {
        Self {
            counters: self.counters,
            sample_freq: self.sample_freq,
            pid: Some(process.pid()),
            prefer_raw_events: self.prefer_raw_events,
        }
    }

    pub fn sample_freq(self, sample_freq: u64) -> Self {
        Self {
            counters: self.counters,
            sample_freq,
            pid: self.pid,
            prefer_raw_events: self.prefer_raw_events,
        }
    }

    pub fn prefer_raw_events(self) -> Self {
        Self {
            counters: self.counters,
            sample_freq: self.sample_freq,
            pid: self.pid,
            prefer_raw_events: true,
        }
    }

    pub fn build(self) -> Result<SamplingDriver, Error> {
        let mut attrs = get_native_counters(&self.counters, self.prefer_raw_events)?;

        for attr in &mut attrs {
            attr.set_exclude_kernel(1);
            attr.set_exclude_hv(1);
            attr.set_exclusive(0);

            attr.sample_freq = self.sample_freq;
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

        let native_handles = bind_counters(&self.counters, &mut attrs, self.pid)?;

        let page_size = unsafe { sysconf(libc::_SC_PAGE_SIZE) } as usize;
        let mmap_pages = 512;

        let mmaps = native_handles
            .iter()
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

        Ok(SamplingDriver {
            native_handles,
            mmaps,
            page_size,
            mmap_pages,
            running: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl Drop for SamplingDriver {
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

impl CounterResult {
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        self.values
            .iter()
            .find(|(c, _)| *c == kind)
            .map(|(_, v)| v)
            .cloned()
    }
}

impl IntoIterator for CounterResult {
    type Item = (Counter, CounterValue);

    type IntoIter = <Vec<(Counter, CounterValue)> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
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

fn bind_counters(
    counters: &[Counter],
    attrs: &mut [perf_event_attr],
    pid: Option<i32>,
) -> Result<Vec<NativeCounterHandle>, Error> {
    let mut handles: Vec<NativeCounterHandle> = vec![];

    for (cntr, attr) in std::iter::zip(counters, attrs) {
        // cycles and instructions are typically fixed counters and thus always on
        match cntr {
            Counter::Cycles | Counter::Instructions => attr.set_pinned(1),
            _ => attr.set_pinned(0),
        };
        let new_fd = unsafe {
            sys::perf_event_open(
                &mut *attr as *mut perf_event_attr,
                pid.unwrap_or(0),
                -1,
                -1,
                0,
            )
        };

        if new_fd < 0 {
            return Err(Error::CounterCreationFail);
        }

        let mut id: u64 = 0;

        let result = unsafe { sys::ioctls::ID(new_fd, &mut id) };
        if result < 0 {
            return Err(Error::CounterCreationFail);
        }

        handles.push(NativeCounterHandle {
            kind: cntr.clone(),
            id,
            fd: new_fd,
        });
    }

    Ok(handles)
}

impl SampleFormat {
    unsafe fn read_from_ptr(ptr: *const u8) -> (*const u8, Self) {
        let sample: Self = std::ptr::read(ptr as *const _);
        let next_ptr = ptr.add(std::mem::size_of::<Self>());
        (next_ptr, sample)
    }

    unsafe fn read_values(ptr: *const u8, nr: u64) -> (*const u8, &'static [EventValue]) {
        let values = std::slice::from_raw_parts(ptr as *const EventValue, nr as usize);
        let next_ptr = ptr.add(std::mem::size_of::<EventValue>() * nr as usize);
        (next_ptr, values)
    }

    unsafe fn read_callchain(ptr: *const u8) -> (*const u8, SmallVec<[u64; 32]>) {
        let nr_callchain = std::ptr::read(ptr as *const u64);
        let callchain_ptr = ptr.add(std::mem::size_of::<u64>());

        let callchain =
            std::slice::from_raw_parts(callchain_ptr as *const u64, nr_callchain as usize)
                .to_smallvec();

        let next_ptr = callchain_ptr.add(std::mem::size_of::<u64>() * nr_callchain as usize);
        (next_ptr, callchain)
    }
}
