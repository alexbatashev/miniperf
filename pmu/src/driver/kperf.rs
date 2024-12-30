#![allow(dead_code)]
// use crate::backends::{Backend, BackendCounters};
// use crate::{CounterKind, CountersGroup};
use dlopen2::wrapper::{Container, WrapperApi};
use libc::*;
// use std::ffi::CStr;
use std::{ffi::CStr, sync::Arc};

use crate::{Counter, Error, Process};

const MAX_COUNTERS: usize = 6;

const KPC_CLASS_FIXED: u32 = 0;
const KPC_CLASS_CONFIGURABLE: u32 = 1;
const KPC_CLASS_POWER: u32 = 2;
const KPC_CLASS_RAWPMU: u32 = 3;

const KPC_CLASS_FIXED_MASK: u32 = 1 << KPC_CLASS_FIXED;
const KPC_CLASS_CONFIGURABLE_MASK: u32 = 1 << KPC_CLASS_CONFIGURABLE;
const KPC_CLASS_POWER_MASK: u32 = 1 << KPC_CLASS_POWER;
const KPC_CLASS_RAWPMU_MASK: u32 = 1 << KPC_CLASS_RAWPMU;

pub enum CountingDriver {
    InProcess(InProcessDriver),
    Sampling,
}

pub struct InProcessDriver {
    kpc_dispatch: Arc<Container<KPCDispatch>>,
    kpep_dispatch: Arc<Container<KPEPDispatch>>,
    db: *const KPepDB,
    cfg: *mut KPepConfig,
}

#[derive(Debug, Clone)]
pub struct CounterValue {
    pub value: u64,
    pub scaling: f64,
}

#[derive(Debug, Clone)]
pub struct CounterResult {
    values: Vec<(Counter, CounterValue)>,
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

impl CountingDriver {
    pub fn new(
        counters: &[Counter],
        pid: Option<&Process>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if pid.is_some() {
            unimplemented!()
        }

        let kpc_dispatch: Container<KPCDispatch> =
            unsafe { Container::load("/System/Library/PrivateFrameworks/kperf.framework/kperf") }?;
        let kpep_dispatch: Container<KPEPDispatch> = unsafe {
            Container::load("/System/Library/PrivateFrameworks/kperfdata.framework/kperfdata")
        }?;

        let kpep_dispatch = Arc::new(kpep_dispatch);

        let mut db: *mut KPepDB = std::ptr::null_mut();
        if unsafe { kpep_dispatch.kpep_db_create(std::ptr::null(), &mut db) } != 0 {
            panic!("Failed to load kpep database");
        }

        let cfg = convert_counters(counters, &kpep_dispatch, db)?;

        Ok(CountingDriver::InProcess(InProcessDriver {
            kpc_dispatch: kpc_dispatch.into(),
            kpep_dispatch,
            db,
            cfg,
        }))
    }

    pub fn start(&mut self) -> Result<(), Error> {
        todo!()
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        todo!()
    }
    pub fn reset(&mut self) -> Result<(), Error> {
        Ok(())
        // todo!()
    }
    pub fn counters(&mut self) -> Result<CounterResult, Box<dyn std::error::Error>> {
        todo!()
    }
}

impl CounterResult {
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        todo!()
    }
}

impl IntoIterator for CounterResult {
    type Item = (Counter, CounterValue);

    type IntoIter = <Vec<(Counter, CounterValue)> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        todo!()
        // self.values.into_iter()
    }
}

pub fn list_software_counters() -> Vec<Counter> {
    vec![]
}

macro_rules! macos_event {
    ($m1_name:expr, $intel_name:expr, $kperf_events:ident, $cfg:ident, $dispatch:expr) => {
        let m1_event = $kperf_events
            .iter()
            .find(|e| event_matches_name(*(*e), $m1_name));
        let intel_event = $kperf_events
            .iter()
            .find(|e| event_matches_name(*(*e), $intel_name));

        let mut event: *mut KPepEvent = m1_event.or(intel_event).unwrap().clone();

        if unsafe {
            $dispatch.kpep_config_add_event($cfg, &mut event, 0, std::ptr::null_mut()) != 0
        } {
            panic!("Failed to add an event");
        }
    };
}

fn convert_counters(
    counters: &[Counter],
    kpep_dispatch: &Arc<Container<KPEPDispatch>>,
    db: *mut KPepDB,
) -> Result<*mut KPepConfig, Error> {
    let mut num_events: size_t = 0;
    if unsafe { kpep_dispatch.kpep_db_events_count(db, &mut num_events) } != 0 {
        panic!()
        // return Err("Failed to count events".to_string());
    }
    let mut kperf_events: Vec<*mut KPepEvent> = Vec::with_capacity(num_events as usize);
    kperf_events.resize(num_events, std::ptr::null_mut());
    if unsafe {
        kpep_dispatch.kpep_db_events(
            db,
            kperf_events.as_mut_ptr(),
            num_events * std::mem::size_of::<*mut u8>(),
        )
    } != 0
    {
        panic!()
        // return Err("Failed to query events".to_string());
    }

    let mut cfg: *mut KPepConfig = std::ptr::null_mut();
    if unsafe { kpep_dispatch.kpep_config_create(db, &mut cfg) } != 0 {
        panic!("Failed to create config");
    }
    if unsafe { kpep_dispatch.kpep_config_force_counters(cfg) != 0 } {
        panic!("Failed to set counters");
    }

    for cntr in counters {
        match cntr {
            Counter::Cycles => {
                macos_event!(
                    "FIXED_CYCLES",
                    "CPU_CLK_UNHALTED.REF_TSC",
                    kperf_events,
                    cfg,
                    kpep_dispatch
                );
            }
            Counter::Instructions => {
                macos_event!(
                    "FIXED_INSTRUCTIONS",
                    "INST_RETIRED.ANY",
                    kperf_events,
                    cfg,
                    kpep_dispatch
                );
            }
            _ => panic!(),
        }
    }

    Ok(cfg)
}

fn event_matches_name(e: *mut KPepEvent, name: &str) -> bool {
    let c_str: &CStr = unsafe { CStr::from_ptr((*e).name) };
    let str_slice: &str = c_str.to_str().unwrap();

    str_slice == name
}
