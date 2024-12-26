#![allow(dead_code)]
// use crate::backends::{Backend, BackendCounters};
// use crate::{CounterKind, CountersGroup};
use dlopen2::wrapper::{Container, WrapperApi};
use libc::*;
// use std::ffi::CStr;
use std::sync::Arc;

use crate::Counter;

const MAX_COUNTERS: usize = 6;

const KPC_CLASS_FIXED: u32 = 0;
const KPC_CLASS_CONFIGURABLE: u32 = 1;
const KPC_CLASS_POWER: u32 = 2;
const KPC_CLASS_RAWPMU: u32 = 3;

const KPC_CLASS_FIXED_MASK: u32 = 1 << KPC_CLASS_FIXED;
const KPC_CLASS_CONFIGURABLE_MASK: u32 = 1 << KPC_CLASS_CONFIGURABLE;
const KPC_CLASS_POWER_MASK: u32 = 1 << KPC_CLASS_POWER;
const KPC_CLASS_RAWPMU_MASK: u32 = 1 << KPC_CLASS_RAWPMU;

pub struct Driver {
    kpc_dispatch: Arc<Container<KPCDispatch>>,
    kpep_dispatch: Arc<Container<KPEPDispatch>>,
    db: *const KPepDB,
}

#[repr(C)]
struct KPepEvent {
    name: *const c_char,
    description: *const c_char,
    errata: *const c_char,
    alias: *const c_char,
    fallback: *const c_char,
    mask: u32,
    number: u8,
    umask: u8,
    reserved: u8,
    is_fixed: u8,
}

#[repr(C)]
struct KPepDB {
    name: *const c_char,
    cpu_id: *const c_char,
    marketing_name: *const c_char,
    plist_data: *const u8,
    event_map: *const u8,
    event_arr: *const KPepEvent,
    fixed_event_arr: *const *const KPepEvent,
    alias_map: *const u8,
    reserved1: size_t,
    reserved2: size_t,
    reserved3: size_t,
    event_count: size_t,
    alias_count: size_t,
    fixed_count_counter: size_t,
    config_counter_count: size_t,
    power_counter_count: size_t,
    architecture: u32,
    fixed_counter_bits: u32,
    config_counter_bits: u32,
    power_counter_bits: u32,
}

#[repr(C)]
struct KPepConfig {
    db: *const KPepDB,
    ev_arr: *const *const KPepEvent,
    ev_map: *const size_t,
    ev_idx: *const size_t,
    flags: *const u32,
    kpc_periods: *const u64,
    event_count: size_t,
    counter_count: size_t,
    classes: u32,
    config_counter: u32,
    power_counter: u32,
    reserved: u32,
}

#[derive(WrapperApi)]
struct KPCDispatch {
    kpc_cpu_string: unsafe extern "C" fn(buf: *mut c_char, buf_size: size_t) -> c_int,
    kpc_pmu_version: unsafe extern "C" fn() -> u32,
    kpc_set_counting: unsafe extern "C" fn(classes: u32) -> c_int,
    kpc_set_thread_counting: unsafe extern "C" fn(classes: u32) -> c_int,
    kpc_set_config: unsafe extern "C" fn(classes: u32, config: *mut u64) -> c_int,
    kpc_get_thread_counters: unsafe extern "C" fn(tid: u32, buf_count: u32, buf: *mut u64) -> c_int,
    kpc_force_all_ctrs_set: unsafe extern "C" fn(val: c_int) -> c_int,
}

#[derive(WrapperApi)]
struct KPEPDispatch {
    kpep_config_create: unsafe extern "C" fn(db: *mut KPepDB, cfg: *mut *mut KPepConfig) -> c_int,
    kpep_config_free: unsafe extern "C" fn(cfg: *mut KPepConfig),
    kpep_db_create: unsafe extern "C" fn(name: *const c_char, db: *mut *mut KPepDB) -> c_int,
    kpep_db_free: unsafe extern "C" fn(db: *mut KPepDB),
    kpep_db_events_count: unsafe extern "C" fn(db: *const KPepDB, count: *mut size_t) -> c_int,
    kpep_db_events:
        unsafe extern "C" fn(db: *const KPepDB, buf: *mut *mut KPepEvent, buf_size: usize) -> c_int,
    kpep_event_name: unsafe extern "C" fn(event: *const KPepEvent, name: *mut *const c_char),
    kpep_event_description: unsafe extern "C" fn(event: *const KPepEvent, desc: *mut *const c_char),
    kpep_config_force_counters: unsafe extern "C" fn(cfg: *mut KPepConfig) -> c_int,
    kpep_config_add_event: unsafe extern "C" fn(
        cfg: *mut KPepConfig,
        evt: *mut *mut KPepEvent,
        flag: u32,
        err: *mut u32,
    ) -> c_int,
    kpep_config_kpc:
        unsafe extern "C" fn(cfg: *mut KPepConfig, buf: *mut u64, buf_size: usize) -> c_int,
    kpep_config_kpc_count:
        unsafe extern "C" fn(cfg: *mut KPepConfig, count_ptr: *mut usize) -> c_int,
    kpep_config_kpc_classes:
        unsafe extern "C" fn(cfg: *mut KPepConfig, classes_ptr: *mut u32) -> c_int,
    kpep_config_kpc_map:
        unsafe extern "C" fn(cfg: *mut KPepConfig, buf: *mut usize, size: usize) -> c_int,
}

struct NativeCounterHandle {
    // pub kind: CounterKind,
    pub reg_id: usize,
}

struct KPerfCounters {
    kpc_dispatch: Arc<Container<KPCDispatch>>,
    kpep_dispatch: Arc<Container<KPEPDispatch>>,
    native_handles: Vec<NativeCounterHandle>,
    counter_values_before: Vec<u64>,
    counter_values_after: Vec<u64>,
    config: *mut KPepConfig,
}

impl Driver {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let kpc_dispatch: Container<KPCDispatch> =
            unsafe { Container::load("/System/Library/PrivateFrameworks/kperf.framework/kperf") }?;
        let kpep_dispatch: Container<KPEPDispatch> = unsafe {
            Container::load("/System/Library/PrivateFrameworks/kperfdata.framework/kperfdata")
        }?;

        let mut db: *mut KPepDB = std::ptr::null_mut();
        if unsafe { kpep_dispatch.kpep_db_create(std::ptr::null(), &mut db) } != 0 {
            panic!("Failed to load kpep database");
        }

        Ok(Driver {
            kpc_dispatch: kpc_dispatch.into(),
            kpep_dispatch: kpep_dispatch.into(),
            db,
        })
    }
}

pub fn list_software_counters() -> Vec<Counter> {
    vec![]
}
