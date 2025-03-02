use std::iter::zip;

use itertools::Itertools;
use perf_event_open_sys::{self as sys, bindings::perf_event_attr};

use crate::{cpu_family, Counter, Error};

use super::NativeCounterHandle;

pub fn direct(
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
            leader: false,
        });
    }

    Ok(handles)
}

pub fn grouped(
    counters: &[Counter],
    attrs: &mut [perf_event_attr],
    pid: Option<i32>,
) -> Result<Vec<NativeCounterHandle>, Error> {
    let cpu_family = cpu_family::get_host_cpu_family();
    let info = cpu_family::find_cpu_family(cpu_family);

    let max_counters_in_group = info.and_then(|info| info.max_counters).unwrap_or(3);

    let mut cycles_attrs = zip(counters, attrs.iter())
        .find(|(cntr, _)| **cntr == Counter::Cycles)
        .map(|(_, attrs)| attrs)
        .cloned()
        .expect("Cycles are required for correct sampling");
    let mut instr_attrs = zip(counters, attrs.iter())
        .find(|(cntr, _)| **cntr == Counter::Instructions)
        .map(|(_, attrs)| attrs)
        .cloned()
        .expect("Instructions are required for correct sampling");

    let chunks = zip(counters, attrs.iter_mut())
        .filter(|(cntr, _)| **cntr != Counter::Cycles && **cntr != Counter::Instructions)
        .chunks(max_counters_in_group);

    let mut handles: Vec<NativeCounterHandle> = vec![];

    for chunk in chunks.into_iter() {
        let cycles_fd =
            unsafe { sys::perf_event_open(&mut cycles_attrs, pid.unwrap_or(0), -1, -1, 0) };

        handles.push(get_native_handle(cycles_fd, Counter::Cycles, true)?);

        let instr_fd =
            unsafe { sys::perf_event_open(&mut instr_attrs, pid.unwrap_or(0), -1, cycles_fd, 0) };

        handles.push(get_native_handle(instr_fd, Counter::Instructions, false)?);

        for (cntr, attrs) in chunk {
            let new_fd =
                unsafe { sys::perf_event_open(&mut *attrs, pid.unwrap_or(0), -1, cycles_fd, 0) };
            handles.push(get_native_handle(new_fd, cntr.clone(), false)?);
        }
    }

    Ok(handles)
}

fn get_native_handle(fd: i32, cntr: Counter, leader: bool) -> Result<NativeCounterHandle, Error> {
    if fd < 0 {
        return Err(Error::CounterCreationFail);
    }

    let mut id: u64 = 0;

    let result = unsafe { sys::ioctls::ID(fd, &mut id) };
    if result < 0 {
        return Err(Error::CounterCreationFail);
    }

    Ok(NativeCounterHandle {
        kind: cntr,
        id,
        fd,
        leader,
    })
}
