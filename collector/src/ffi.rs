use smallvec::smallvec;
use std::ffi::CStr;

use mperf_data::{CallFrame, Event, EventType, Location};

use crate::{
    get_next_id, get_string_id, get_timestamp, profiling_enabled, roofline_instrumentation_enabled,
    send_event,
};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LoopInfo {
    line: u32,
    filename: *const libc::c_char,
    func_name: *const libc::c_char,
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct SafeLoopInfo {
    line: u32,
    filename: String,
    func_name: String,
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
        func_name: CStr::from_ptr(info.func_name).to_str().unwrap().to_string(),
    };

    let mut handle = Box::new(LoopHandle {
        id,
        timestamp: 0,
        info,
    });

    handle.timestamp = get_timestamp();

    // FIXME we should use the full stack frame instead
    let filename = get_string_id(&handle.info.filename);
    let func_name = get_string_id(&handle.info.func_name);

    let start_frame = CallFrame::Location(Location {
        function_name: func_name as u128,
        file_name: filename as u128,
        line: handle.info.line,
    });

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
        name: 0,
        timestamp: handle.timestamp,
        callstack: smallvec![start_frame],
    };

    send_event(start_event).expect("failed to send start event");

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

    let handle = unsafe { handle.as_ref() }.unwrap();

    let timestamp = get_timestamp();

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
        name: 0,
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
    let stats = unsafe { stats.as_ref().cloned().unwrap_or_default() };

    let handle = unsafe { handle.as_ref() }.unwrap();

    let timestamp = get_timestamp();

    let send_counter_event = |ty: EventType, value: u64| {
        let event = Event {
            unique_id: get_next_id(),
            correlation_id: 0,
            parent_id: handle.id,
            ty,
            thread_id: libc::gettid() as u32,
            process_id: std::process::id(),
            time_enabled: 0,
            time_running: 0,
            value,
            name: 0,
            timestamp,
            callstack: smallvec![],
        };

        send_event(event).expect("failed to send loop end event");
    };

    send_counter_event(EventType::RooflineBytesLoad, stats.bytes_load);
    send_counter_event(EventType::RooflineBytesStore, stats.bytes_store);
    send_counter_event(EventType::RooflineScalarIntOps, stats.scalar_int_ops);
    send_counter_event(EventType::RooflineScalarFloatOps, stats.scalar_float_ops);
    send_counter_event(EventType::RooflineVectorIntOps, stats.vector_int_ops);
    send_counter_event(EventType::RooflineVectorFloatOps, stats.vector_float_ops);
    send_counter_event(EventType::RooflineVectorDoubleOps, stats.vector_double_ops);
}
