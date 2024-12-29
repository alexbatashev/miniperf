use perf_event_open_sys::bindings::{perf_event_attr, PERF_FLAG_FD_CLOEXEC};
use perf_event_open_sys::{self as sys, bindings::PERF_SAMPLE_IDENTIFIER};

use crate::{Counter, Error, Process};

pub struct CountingDriver {
    native_handles: Vec<NativeCounterHandle>,
}

struct NativeCounterHandle {
    pub kind: Counter,
    pub fd: i32,
}

#[repr(C)]
struct ReadFormat {
    nr: u64,
    time_enabled: u64,
    time_running: u64,
    values: [EventValue; 0],
}

#[repr(C)]
struct EventValue {
    value: u64,
    id: u64,
}

pub fn list_software_counters() -> Vec<Counter> {
    vec![]
}

impl CountingDriver {
    pub fn new(
        counters: &[Counter],
        process: Option<&Process>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut attrs = get_native_counters(counters)?;

        for attr in &mut attrs {
            attr.set_exclude_kernel(1);
            attr.set_exclude_hv(1);
            attr.set_inherit(1);
            attr.set_pinned(0);
            attr.set_exclusive(0);
            attr.sample_type = PERF_SAMPLE_IDENTIFIER as u64;
            if process.is_some() {
                attr.set_enable_on_exec(1);
            }
        }

        let pid = process.map(|p| p.pid());

        let native_handles = bind_events(counters, &mut attrs, pid)?;

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

    pub fn get_counters(&mut self) -> Result<Vec<(Counter, u64)>, Box<dyn std::error::Error>> {
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
                return Err(std::io::Error::last_os_error().into());
            }

            let header = unsafe { &*(buffer.as_ptr() as *const ReadFormat) };

            let values = unsafe {
                std::slice::from_raw_parts(
                    (buffer.as_ptr() as *const ReadFormat).add(1) as *const EventValue,
                    self.native_handles.len(),
                )
            };

            let value = &values[0];

            let scaled_value = if header.time_running > 0 {
                let scaling_factor = (header.time_enabled as f64) / (header.time_running as f64);
                (value.value as f64 * scaling_factor) as u64
            } else {
                value.value
            };
            scaled_values.push((handle.kind.clone(), scaled_value));
        }

        Ok(scaled_values)
    }
}

fn get_native_counters(counters: &[Counter]) -> Result<Vec<perf_event_attr>, Error> {
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
                _ => todo!(),
            }

            attrs
        })
        .collect::<Vec<_>>();

    Ok(attrs)
}

fn bind_events(
    counters: &[Counter],
    attrs: &mut [perf_event_attr],
    pid: Option<i32>,
) -> Result<Vec<NativeCounterHandle>, Error> {
    let mut handles: Vec<NativeCounterHandle> = vec![];

    for (cntr, attr) in std::iter::zip(counters, attrs) {
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

        handles.push(NativeCounterHandle {
            kind: cntr.clone(),
            fd: new_fd,
        });
    }

    Ok(handles)
}
