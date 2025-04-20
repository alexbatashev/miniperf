use std::{ffi::CStr, sync::atomic::AtomicU64};

use perf_event_open_sys::bindings::{
    perf_event_header, perf_event_mmap_page, PERF_RECORD_MMAP, PERF_RECORD_SAMPLE,
};
use smallvec::{SmallVec, ToSmallVec};

pub struct Records {
    metadata: *mut perf_event_mmap_page,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct EventValue {
    pub value: u64,
    pub id: u64,
}

pub enum MmapRecord {
    Sample {
        ip: u64,
        pid: u32,
        tid: u32,
        cpu: u32,
        time: u64,
        time_enabled: u64,
        time_running: u64,
        values: SmallVec<[EventValue; 8]>,
        callstack: SmallVec<[u64; 32]>,
    },
    Address {
        pid: u32,
        start: u64,
        len: u64,
        offset: u64,
        filename: String,
    },
    Unknown,
}

/// Internal structure for reading from mmap ring buffer
#[repr(C)]
pub struct SampleFormat {
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
pub struct ReadFormat {
    pub nr: u64,
    pub time_enabled: u64,
    pub time_running: u64,
}

#[repr(C)]
struct ProcMmap {
    header: perf_event_header,
    pid: u32,
    tid: u32,
    addr: u64,
    len: u64,
    pgoff: u64,
    // Filename
}

impl Records {
    pub fn from_ptr(ptr: *mut u8) -> Records {
        Records {
            metadata: ptr as *mut perf_event_mmap_page,
        }
    }

    unsafe fn data_head(&self) -> u64 {
        let atomic = AtomicU64::from_ptr(&mut (*self.metadata).data_head as *mut u64);
        atomic.load(std::sync::atomic::Ordering::Acquire)
    }

    unsafe fn data_tail(&self) -> u64 {
        let atomic = AtomicU64::from_ptr(&mut (*self.metadata).data_tail as *mut u64);
        atomic.load(std::sync::atomic::Ordering::Acquire)
    }

    fn data_size(&self) -> usize {
        unsafe { (*self.metadata).data_size as usize }
    }

    fn data_offset(&self) -> usize {
        unsafe { (*self.metadata).data_offset as usize }
    }

    fn update_tail(&mut self, offset: usize) {
        unsafe {
            let atomic = AtomicU64::from_ptr(&mut (*self.metadata).data_tail as *mut u64);
            atomic.fetch_add(offset as u64, std::sync::atomic::Ordering::Release);
        }
    }
}

impl Iterator for Records {
    type Item = MmapRecord;

    fn next(&mut self) -> Option<Self::Item> {
        let data_head = unsafe { self.data_head() };
        let data_tail = unsafe { self.data_tail() };

        if data_tail == data_head {
            return None;
        }

        if data_tail + std::mem::size_of::<perf_event_header>() as u64 > data_head {
            return None;
        }

        let mmap_base_ptr = self.metadata as *mut u8;

        let ptr_offset = self.data_offset() + (data_tail % self.data_size() as u64) as usize;
        let event_ptr = unsafe { mmap_base_ptr.add(ptr_offset) };

        let header = unsafe { &*(event_ptr as *const perf_event_header) };

        if header.size == 0 {
            return None;
        }

        let record = if header.type_ == PERF_RECORD_SAMPLE {
            let sample_format = unsafe { SampleFormat::read_from_ptr(event_ptr as *const u8) };
            let values = unsafe { sample_format.read_values(event_ptr as *const u8) };
            let callstack = unsafe { sample_format.read_callchain(event_ptr as *const u8) };

            MmapRecord::Sample {
                ip: sample_format.ip,
                pid: sample_format.pid,
                tid: sample_format.tid,
                cpu: sample_format.cpu,
                time: sample_format.time,
                time_enabled: sample_format.read.time_enabled,
                time_running: sample_format.read.time_running,
                values,
                callstack,
            }
        } else if header.type_ == PERF_RECORD_MMAP {
            let mmap_record = unsafe { ProcMmap::read_from_ptr(event_ptr as *const u8) };
            let filename = unsafe { ProcMmap::filename(event_ptr as *const u8) };
            MmapRecord::Address {
                pid: mmap_record.pid,
                start: mmap_record.addr,
                len: mmap_record.len,
                offset: mmap_record.pgoff,
                filename,
            }
        } else {
            MmapRecord::Unknown
        };

        self.update_tail(header.size as usize);

        Some(record)
    }
}

impl SampleFormat {
    unsafe fn read_from_ptr(ptr: *const u8) -> Self {
        std::ptr::read_volatile(ptr as *const _)
    }

    unsafe fn read_values(&self, ptr: *const u8) -> SmallVec<[EventValue; 8]> {
        std::slice::from_raw_parts(
            ptr.add(std::mem::size_of::<SampleFormat>()) as *const EventValue,
            self.read.nr as usize,
        )
        .to_smallvec()
    }

    unsafe fn read_callchain(&self, ptr: *const u8) -> SmallVec<[u64; 32]> {
        let base_offset = std::mem::size_of::<SampleFormat>()
            + self.read.nr as usize * std::mem::size_of::<EventValue>();

        let nr_callchain = std::ptr::read(ptr.add(base_offset) as *const u64);

        let callchain_ptr = ptr.add(base_offset + std::mem::size_of::<u64>());

        let slice = std::slice::from_raw_parts(callchain_ptr as *const u64, nr_callchain as usize);

        if slice.len() > 1 {
            slice[1..].to_smallvec()
        } else {
            slice.to_smallvec()
        }
    }
}

impl ProcMmap {
    unsafe fn read_from_ptr(ptr: *const u8) -> Self {
        std::ptr::read_volatile(ptr as *const _)
    }

    unsafe fn filename(ptr: *const u8) -> String {
        CStr::from_ptr(ptr.add(std::mem::size_of::<Self>()) as *const libc::c_char)
            .to_str()
            .expect("assume kernel does not corrupt data")
            .to_owned()
    }
}
