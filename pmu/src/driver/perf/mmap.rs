use std::{ffi::CStr, sync::atomic::AtomicU64};

use perf_event_open_sys::bindings::{
    perf_event_header, perf_event_mmap_page, PERF_RECORD_MMAP, PERF_RECORD_SAMPLE,
};
use smallvec::{SmallVec, ToSmallVec};

pub struct Records {
    metadata: *mut perf_event_mmap_page,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub(crate) struct EventValue {
    pub value: u64,
    pub id: u64,
}

#[derive(Debug)]
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

#[cfg(test)]
mod test {
    use super::Records;

    #[test]
    fn basic_test() {
        let mut test_data: Vec<u8> = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 34, 0, 0, 0, 0, 0, 0, 0, 245, 26, 2, 0, 0, 0, 0, 0, 48, 153, 4,
            0, 0, 0, 0, 0, 48, 153, 4, 0, 0, 0, 0, 0, 30, 0, 0, 0, 0, 0, 0, 0, 48, 0, 31, 0, 153,
            171, 170, 42, 55, 105, 129, 224, 161, 252, 255, 255, 247, 227, 229, 218, 247, 255, 255,
            255, 96, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 10, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0,
            2, 0, 232, 0, 137, 223, 38, 43, 9, 127, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 130, 103,
            96, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0, 32, 173, 0, 0, 0, 0, 0, 0, 32, 173, 0, 0, 0, 0, 0, 0, 44, 44, 0, 0, 0, 0, 0,
            0, 58, 0, 0, 0, 0, 0, 0, 0, 17, 0, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255,
            255, 137, 223, 38, 43, 9, 127, 0, 0, 90, 150, 38, 43, 9, 127, 0, 0, 62, 176, 38, 43, 9,
            127, 0, 0, 124, 237, 38, 43, 9, 127, 0, 0, 35, 69, 38, 43, 9, 127, 0, 0, 160, 228, 38,
            43, 9, 127, 0, 0, 35, 69, 38, 43, 9, 127, 0, 0, 4, 233, 38, 43, 9, 127, 0, 0, 244, 156,
            199, 42, 9, 127, 0, 0, 35, 69, 38, 43, 9, 127, 0, 0, 121, 70, 38, 43, 9, 127, 0, 0,
            227, 151, 199, 42, 9, 127, 0, 0, 175, 157, 199, 42, 9, 127, 0, 0, 101, 19, 64, 0, 0, 0,
            0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 32, 96,
            0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 96, 37, 43, 9, 127, 0, 0, 0, 80, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 47, 104, 111, 109, 101, 47, 97, 108, 101, 120, 47, 108, 105,
            98, 104, 101, 108, 108, 111, 46, 115, 111, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 100,
            193, 96, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
            0, 2, 0, 96, 0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 112, 37, 43, 9, 127, 0, 0, 0, 16, 0,
            0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 47, 104, 111, 109, 101, 47, 97, 108, 101, 120,
            47, 108, 105, 98, 104, 101, 108, 108, 111, 46, 115, 111, 0, 0, 252, 57, 0, 0, 252, 57,
            0, 0, 210, 37, 97, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0,
            1, 0, 0, 0, 2, 32, 96, 0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 128, 37, 43, 9, 127, 0, 0,
            0, 48, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 47, 104, 111, 109, 101, 47, 97, 108,
            101, 120, 47, 108, 105, 98, 104, 101, 108, 108, 111, 46, 115, 111, 0, 0, 252, 57, 0, 0,
            252, 57, 0, 0, 76, 81, 97, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0,
            0, 0, 0, 1, 0, 0, 0, 2, 32, 96, 0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 144, 37, 43, 9,
            127, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 47, 104, 111, 109, 101,
            47, 97, 108, 101, 120, 47, 108, 105, 98, 104, 101, 108, 108, 111, 46, 115, 111, 0, 0,
            252, 57, 0, 0, 252, 57, 0, 0, 158, 119, 97, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0,
            38, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 240, 0, 201, 174, 38, 43, 9, 127, 0, 0, 252,
            57, 0, 0, 252, 57, 0, 0, 38, 239, 97, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0,
            0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 52, 36, 2, 0, 0, 0, 0, 0, 52, 36, 2, 0, 0, 0,
            0, 0, 251, 81, 0, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0,
            254, 255, 255, 255, 255, 255, 255, 201, 174, 38, 43, 9, 127, 0, 0, 189, 88, 38, 43, 9,
            127, 0, 0, 35, 69, 38, 43, 9, 127, 0, 0, 40, 93, 38, 43, 9, 127, 0, 0, 229, 237, 38,
            43, 9, 127, 0, 0, 35, 69, 38, 43, 9, 127, 0, 0, 160, 228, 38, 43, 9, 127, 0, 0, 35, 69,
            38, 43, 9, 127, 0, 0, 4, 233, 38, 43, 9, 127, 0, 0, 244, 156, 199, 42, 9, 127, 0, 0,
            35, 69, 38, 43, 9, 127, 0, 0, 121, 70, 38, 43, 9, 127, 0, 0, 227, 151, 199, 42, 9, 127,
            0, 0, 175, 157, 199, 42, 9, 127, 0, 0, 101, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9,
            127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 232, 0, 83, 190, 38, 43, 9,
            127, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 100, 36, 98, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0,
            0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 32, 86, 2, 0, 0, 0, 0, 0, 32,
            86, 2, 0, 0, 0, 0, 0, 6, 121, 0, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 17, 0, 0, 0,
            0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 83, 190, 38, 43, 9, 127, 0, 0, 174,
            202, 38, 43, 9, 127, 0, 0, 246, 14, 39, 43, 9, 127, 0, 0, 6, 240, 38, 43, 9, 127, 0, 0,
            35, 69, 38, 43, 9, 127, 0, 0, 160, 228, 38, 43, 9, 127, 0, 0, 35, 69, 38, 43, 9, 127,
            0, 0, 4, 233, 38, 43, 9, 127, 0, 0, 244, 156, 199, 42, 9, 127, 0, 0, 35, 69, 38, 43, 9,
            127, 0, 0, 121, 70, 38, 43, 9, 127, 0, 0, 227, 151, 199, 42, 9, 127, 0, 0, 175, 157,
            199, 42, 9, 127, 0, 0, 101, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133,
            17, 64, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 32, 96, 0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 144,
            37, 43, 9, 127, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 47, 104, 111,
            109, 101, 47, 97, 108, 101, 120, 47, 108, 105, 98, 104, 101, 108, 108, 111, 46, 115,
            111, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 116, 100, 98, 250, 85, 3, 0, 0, 58, 0, 0, 0,
            0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 144, 0, 190, 152, 199, 42, 9,
            127, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 0, 151, 98, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0,
            0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 166, 192, 2, 0, 0, 0, 0, 0,
            166, 192, 2, 0, 0, 0, 0, 0, 6, 160, 0, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0,
            0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 190, 152, 199, 42, 9, 127, 0, 0,
            175, 157, 199, 42, 9, 127, 0, 0, 101, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0,
            0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 1, 0, 216, 0, 83, 229, 36, 164, 255, 255,
            255, 255, 252, 57, 0, 0, 252, 57, 0, 0, 244, 212, 98, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0,
            0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 70, 32, 3, 0, 0, 0, 0, 0, 70,
            32, 3, 0, 0, 0, 0, 0, 108, 195, 0, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 15, 0, 0, 0,
            0, 0, 0, 0, 128, 255, 255, 255, 255, 255, 255, 255, 83, 229, 36, 164, 255, 255, 255,
            255, 166, 18, 64, 164, 255, 255, 255, 255, 0, 254, 255, 255, 255, 255, 255, 255, 16,
            129, 198, 42, 9, 127, 0, 0, 200, 85, 199, 42, 9, 127, 0, 0, 232, 96, 199, 42, 9, 127,
            0, 0, 73, 52, 196, 42, 9, 127, 0, 0, 12, 53, 196, 42, 9, 127, 0, 0, 155, 237, 196, 42,
            9, 127, 0, 0, 243, 43, 196, 42, 9, 127, 0, 0, 33, 113, 37, 43, 9, 127, 0, 0, 121, 19,
            64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0,
            0, 2, 0, 128, 0, 132, 19, 64, 0, 0, 0, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 138, 62, 99,
            250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
            0, 0, 90, 102, 3, 0, 0, 0, 0, 0, 90, 102, 3, 0, 0, 0, 0, 0, 32, 238, 0, 0, 0, 0, 0, 0,
            58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255,
            132, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0,
            9, 0, 0, 0, 2, 0, 128, 0, 132, 19, 64, 0, 0, 0, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0,
            110, 100, 99, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0,
            0, 0, 0, 0, 0, 0, 188, 139, 3, 0, 0, 0, 0, 0, 188, 139, 3, 0, 0, 0, 0, 0, 43, 21, 1, 0,
            0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255,
            255, 255, 255, 132, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64,
            0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0, 132, 19, 64, 0, 0, 0, 0, 0, 252, 57, 0, 0,
            252, 57, 0, 0, 158, 137, 99, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0,
            0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 226, 176, 3, 0, 0, 0, 0, 0, 226, 176, 3, 0, 0, 0, 0,
            0, 64, 60, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 254,
            255, 255, 255, 255, 255, 255, 132, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0,
            133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0, 132, 19, 64, 0, 0, 0, 0, 0, 252,
            57, 0, 0, 252, 57, 0, 0, 176, 174, 99, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 38,
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 244, 213, 3, 0, 0, 0, 0, 0, 244, 213, 3,
            0, 0, 0, 0, 0, 76, 99, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0,
            0, 0, 254, 255, 255, 255, 255, 255, 255, 132, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42, 9,
            127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0, 132, 19, 64, 0, 0, 0,
            0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 238, 222, 99, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0,
            0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 80, 6, 4, 0, 0, 0, 0, 0, 80, 6,
            4, 0, 0, 0, 0, 0, 88, 138, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0,
            0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 132, 19, 64, 0, 0, 0, 0, 0, 11, 3, 193, 42,
            9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0, 132, 19, 64, 0, 0,
            0, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 70, 4, 100, 250, 85, 3, 0, 0, 58, 0, 0, 0, 0, 0,
            0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 138, 43, 4, 0, 0, 0, 0, 0, 138,
            43, 4, 0, 0, 0, 0, 0, 109, 177, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0,
            0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 132, 19, 64, 0, 0, 0, 0, 0, 11, 3,
            193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0, 132, 19,
            64, 0, 0, 0, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 108, 41, 100, 250, 85, 3, 0, 0, 58, 0,
            0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 176, 80, 4, 0, 0, 0,
            0, 0, 176, 80, 4, 0, 0, 0, 0, 0, 121, 216, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0, 0, 0, 0, 0,
            4, 0, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 132, 19, 64, 0, 0, 0, 0,
            0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0, 128, 0,
            132, 19, 64, 0, 0, 0, 0, 0, 252, 57, 0, 0, 252, 57, 0, 0, 156, 78, 100, 250, 85, 3, 0,
            0, 58, 0, 0, 0, 0, 0, 0, 0, 38, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 224, 117,
            4, 0, 0, 0, 0, 0, 224, 117, 4, 0, 0, 0, 0, 0, 142, 255, 1, 0, 0, 0, 0, 0, 58, 0, 0, 0,
            0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 132, 19, 64,
            0, 0, 0, 0, 0, 11, 3, 193, 42, 9, 127, 0, 0, 133, 17, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];

        let records = Records::from_ptr(test_data.as_mut_ptr());
        let decoded = records.into_iter().collect::<Vec<_>>();

        insta::assert_debug_snapshot!(decoded);
    }
}
