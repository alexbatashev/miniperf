use smallvec::smallvec;
use std::{
    ffi::CStr,
    time::{SystemTime, UNIX_EPOCH},
};

use mperf_data::{Event, EventType};

use crate::{get_next_id, profiling_enabled, roofline_instrumentation_enabled, send_event};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LoopInfo {
    line: u32,
    filename: *const libc::c_char,
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct SafeLoopInfo {
    line: u32,
    filename: String,
}

#[derive(Default, Debug, Clone, Copy)]
#[repr(C)]
pub struct LoopStats {
    trip_count: u64,
    bytes_load: u64,
    bytes_store: u64,
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
}

#[allow(dead_code)]
pub struct LoopHandle {
    id: u128,
    timestamp: u64,
    info: SafeLoopInfo,
}

/// # Safety
/// Shut up, clippy. There's nothing safe about what we do.
#[no_mangle]
pub unsafe extern "C" fn mperf_roofline_internal_notify_loop_begin(
    info: *const LoopInfo,
) -> *mut LoopHandle {
    if !profiling_enabled() {
        return std::ptr::null_mut();
    }
    let id = crate::get_next_id();
    let info = unsafe { info.as_ref().unwrap() };

    let info = SafeLoopInfo {
        line: info.line,
        filename: CStr::from_ptr(info.filename).to_str().unwrap().to_string(),
    };

    let mut handle = Box::new(LoopHandle {
        id,
        timestamp: 0,
        info,
    });

    let time = SystemTime::now();
    handle.timestamp = time
        .duration_since(UNIX_EPOCH)
        .expect("failed to get timestamp")
        .as_millis() as u64;

    Box::leak(handle)
}

#[no_mangle]
pub extern "C" fn mperf_roofline_internal_is_instrumented_profiling() -> i32 {
    if profiling_enabled() && roofline_instrumentation_enabled() {
        1
    } else {
        0
    }
}

/// # Safety
/// Shut up, clippy. There's nothing safe about what we do.
#[no_mangle]
pub unsafe extern "C" fn mperf_roofline_internal_notify_loop_end(handle: *mut LoopHandle) {
    if !profiling_enabled() {
        return;
    }
    let time = SystemTime::now();

    let handle = unsafe { handle.as_ref() }.unwrap();

    let start_event = Event {
        unique_id: handle.id,
        correlation_id: 0,
        parent_id: 0,
        ty: EventType::RooflineLoopStart,
        thread_id: libc::gettid() as u32,
        process_id: std::process::id(),
        time_enabled: 0,
        time_running: 0,
        value: 0,
        timestamp: handle.timestamp,
        callstack: smallvec![],
    };

    send_event(start_event).expect("failed to send start event");

    let timestamp = time
        .duration_since(UNIX_EPOCH)
        .expect("failed to get time")
        .as_millis() as u64;

    let event = Event {
        unique_id: get_next_id(),
        correlation_id: handle.id,
        parent_id: 0,
        ty: EventType::RooflineLoopEnd,
        thread_id: libc::gettid() as u32,
        process_id: std::process::id(),
        time_enabled: 0,
        time_running: 0,
        value: 0,
        timestamp,
        callstack: smallvec![],
    };

    send_event(event).expect("failed to send loop end event");
}

/// # Safety
/// Shut up, clippy. There's nothing safe about what we do.
#[no_mangle]
pub unsafe extern "C" fn mperf_roofline_internal_notify_loop_stats(
    handle: *mut LoopHandle,
    stats: *const LoopStats,
) {
    if !profiling_enabled() {
        return;
    }
    let time = SystemTime::now();
    let stats = unsafe { stats.as_ref().cloned().unwrap_or_default() };

    let handle = unsafe { handle.as_ref() }.unwrap();

    let timestamp = time
        .duration_since(UNIX_EPOCH)
        .expect("failed to get time")
        .as_millis() as u64;

    let send_end_event = |ty: EventType, value: u64| {
        let event = Event {
            unique_id: get_next_id(),
            correlation_id: handle.id,
            parent_id: 0,
            ty,
            thread_id: libc::gettid() as u32,
            process_id: std::process::id(),
            time_enabled: 0,
            time_running: 0,
            value,
            timestamp,
            callstack: smallvec![],
        };

        send_event(event).expect("failed to send loop end event");
    };

    send_end_event(EventType::RooflineBytesLoad, stats.bytes_load);
    send_end_event(EventType::RooflineBytesStore, stats.bytes_store);
    send_end_event(EventType::RooflineScalarIntOps, stats.scalar_int_ops);
    send_end_event(EventType::RooflineScalarFloatOps, stats.scalar_float_ops);
    send_end_event(EventType::RooflineVectorIntOps, stats.vector_int_ops);
    send_end_event(EventType::RooflineVectorFloatOps, stats.vector_float_ops);
    send_end_event(EventType::RooflineVectorDoubleOps, stats.vector_double_ops);
}
