use std::ffi::CStr;

#[repr(C)]
pub struct LoopInfo {
    line: u32,
    filename: *const CStr,
}

#[repr(C)]
pub struct LoopStats {
    trip_count: u64,
    scalar_int_loads: u64,
    scalar_int_stores: u64,
    scalar_int_ops: u64,
    scalar_float_loads: u64,
    scalar_float_stores: u64,
    scalar_float_ops: u64,
    scalar_double_loads: u64,
    scalar_double_stores: u64,
    scalar_double_ops: u64,
    vector_int_loads: u64,
    vector_int_stores: u64,
    vector_int_ops: u64,
    vector_float_loads: u64,
    vector_float_stores: u64,
    vector_float_ops: u64,
    vector_double_loads: u64,
    vector_double_stores: u64,
    vector_double_ops: u64,
}

pub struct LoopHandle {
    id: u64,
    driver: Option<Box<pmu::CountingDriver>>,
}

pub extern "C" fn mperf_notify_loop_begin(info: *const LoopInfo) -> *mut LoopHandle {
    std::ptr::null_mut()
}

pub extern "C" fn mperf_notify_loop_end(id: u64, stats: LoopStats) {

} 
