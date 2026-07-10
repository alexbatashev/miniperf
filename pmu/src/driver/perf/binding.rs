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
            close_handles(&handles);
            return Err(Error::perf_event_open(cntr, None));
        }

        let mut id: u64 = 0;

        let result = unsafe { sys::ioctls::ID(new_fd, &mut id) };
        if result < 0 {
            let error = Error::perf_ioctl("ID", cntr);
            unsafe { libc::close(new_fd) };
            close_handles(&handles);
            return Err(error);
        }

        handles.push(NativeCounterHandle {
            kind: cntr.clone(),
            core: None,
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

    let leader = info.and_then(|info| info.leader_event.clone());

    let leader_cntr = counters.iter().find(|cntr| match (cntr, leader.as_ref()) {
        (Counter::Custom(name), Some(leader)) => name == leader,
        _ => false,
    });

    let max_counters_in_group = info.and_then(|info| info.max_counters).unwrap_or_else(|| {
        if leader.is_some() {
            2
        } else {
            3
        }
    });

    let mut cycles_attrs = zip(counters, attrs.iter())
        .find(|(cntr, _)| **cntr == Counter::Cycles)
        .map(|(_, attrs)| attrs)
        .cloned()
        .ok_or_else(|| {
            Error::InvalidConfiguration("cycles are required for hardware sampling".to_owned())
        })?;
    let mut instr_attrs = zip(counters, attrs.iter())
        .find(|(cntr, _)| **cntr == Counter::Instructions)
        .map(|(_, attrs)| attrs)
        .cloned()
        .ok_or_else(|| {
            Error::InvalidConfiguration(
                "instructions are required for hardware sampling".to_owned(),
            )
        })?;

    let leader_attrs = zip(counters, attrs.iter())
        .find(|(cntr, _)| leader_cntr == Some(*cntr))
        .map(|(_, attrs)| attrs)
        .cloned();

    let mut sw_counters = zip(counters, attrs.iter().cloned())
        .filter(|(cntr, _)| cntr.is_software())
        .collect::<Vec<_>>();

    // Filter out Cycles, Instructions and group leader (if any)
    let chunks = zip(counters, attrs.iter_mut())
        .filter(|(cntr, _)| {
            **cntr != Counter::Cycles
                && **cntr != Counter::Instructions
                && !cntr.is_software()
                && leader_cntr != Some(*cntr)
        })
        .chunks(max_counters_in_group);

    let mut handles: Vec<NativeCounterHandle> = vec![];

    for chunk in chunks.into_iter() {
        let cycles_leader_fd = if leader.is_some() {
            let mut leader_attr = leader_attrs.ok_or_else(|| {
                Error::InvalidConfiguration("configured sampling leader is missing".to_owned())
            })?;
            let leader_counter = leader_cntr.ok_or_else(|| {
                Error::InvalidConfiguration("configured sampling leader is missing".to_owned())
            })?;
            let leader_fd =
                unsafe { sys::perf_event_open(&mut leader_attr, pid.unwrap_or(0), -1, -1, 0) };
            push_handle(&mut handles, leader_fd, leader_counter.clone(), true)?;
            leader_fd
        } else {
            -1
        };

        let cycles_fd = unsafe {
            sys::perf_event_open(&mut cycles_attrs, pid.unwrap_or(0), -1, cycles_leader_fd, 0)
        };

        let leader_fd = if leader.is_some() {
            cycles_leader_fd
        } else {
            cycles_fd
        };

        push_handle(&mut handles, cycles_fd, Counter::Cycles, leader.is_none())?;

        let instr_fd =
            unsafe { sys::perf_event_open(&mut instr_attrs, pid.unwrap_or(0), -1, leader_fd, 0) };

        push_handle(&mut handles, instr_fd, Counter::Instructions, false)?;

        for (cntr, attrs) in chunk {
            let new_fd =
                unsafe { sys::perf_event_open(&mut *attrs, pid.unwrap_or(0), -1, leader_fd, 0) };
            push_handle(&mut handles, new_fd, cntr.clone(), false)?;
        }

        for (cntr, attrs) in &mut sw_counters {
            let new_fd =
                unsafe { sys::perf_event_open(&mut *attrs, pid.unwrap_or(0), -1, leader_fd, 0) };
            push_handle(&mut handles, new_fd, cntr.clone(), false)?;
        }
    }

    Ok(handles)
}

/// Build a single software-event sampling group used when the hardware PMU is
/// unavailable. `cpu-clock` is the group leader and therefore owns the mmap
/// ring buffer that carries samples and grouped counter reads.
pub fn grouped_software(
    counters: &[Counter],
    attrs: &mut [perf_event_attr],
    pid: Option<i32>,
) -> Result<Vec<NativeCounterHandle>, Error> {
    let Some(leader_index) = counters
        .iter()
        .position(|counter| *counter == Counter::CpuClock)
    else {
        return Err(Error::InvalidConfiguration(
            "software sampling fallback requires cpu_clock".to_owned(),
        ));
    };

    let mut handles = Vec::with_capacity(counters.len());
    let leader_fd =
        unsafe { sys::perf_event_open(&mut attrs[leader_index], pid.unwrap_or(0), -1, -1, 0) };
    push_handle(&mut handles, leader_fd, Counter::CpuClock, true)?;

    for (index, (counter, attr)) in zip(counters, attrs).enumerate() {
        if index == leader_index {
            continue;
        }
        let fd = unsafe { sys::perf_event_open(attr, pid.unwrap_or(0), -1, leader_fd, 0) };
        push_handle(&mut handles, fd, counter.clone(), false)?;
    }

    Ok(handles)
}

/// Open one coherent group containing every requested self-monitoring event.
/// The first event is the leader and owns the sampling mmap buffer.
pub fn grouped_all(
    counters: &[Counter],
    attrs: &mut [perf_event_attr],
    pid: Option<i32>,
) -> Result<Vec<NativeCounterHandle>, Error> {
    if counters.is_empty() || counters.len() != attrs.len() {
        return Err(Error::InvalidConfiguration(
            "a sampling group requires matching non-empty counters and attributes".to_owned(),
        ));
    }

    let mut handles = Vec::with_capacity(counters.len());
    let leader_fd = unsafe { sys::perf_event_open(&mut attrs[0], pid.unwrap_or(0), -1, -1, 0) };
    push_handle(&mut handles, leader_fd, counters[0].clone(), true)?;
    for (counter, attr) in zip(&counters[1..], &mut attrs[1..]) {
        let fd = unsafe { sys::perf_event_open(attr, pid.unwrap_or(0), -1, leader_fd, 0) };
        push_handle(&mut handles, fd, counter.clone(), false)?;
    }
    Ok(handles)
}

fn push_handle(
    handles: &mut Vec<NativeCounterHandle>,
    fd: i32,
    counter: Counter,
    leader: bool,
) -> Result<(), Error> {
    match get_native_handle(fd, counter, leader) {
        Ok(handle) => {
            handles.push(handle);
            Ok(())
        }
        Err(error) => {
            close_handles(handles);
            handles.clear();
            Err(error)
        }
    }
}

fn get_native_handle(fd: i32, cntr: Counter, leader: bool) -> Result<NativeCounterHandle, Error> {
    if fd < 0 {
        return Err(Error::perf_event_open(&cntr, None));
    }

    let mut id: u64 = 0;

    let result = unsafe { sys::ioctls::ID(fd, &mut id) };
    if result < 0 {
        let error = Error::perf_ioctl("ID", &cntr);
        unsafe { libc::close(fd) };
        return Err(error);
    }

    Ok(NativeCounterHandle {
        kind: cntr,
        core: None,
        id,
        fd,
        leader,
    })
}

fn close_handles(handles: &[NativeCounterHandle]) {
    for handle in handles {
        unsafe { libc::close(handle.fd) };
    }
}
