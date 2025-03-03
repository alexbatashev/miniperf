#![allow(dead_code)]
// use crate::backends::{Backend, BackendCounters};
// use crate::{CounterKind, CountersGroup};
use dlopen2::wrapper::{Container, WrapperApi};
use libc::*;
// use std::ffi::CStr;
use std::{ffi::CStr, sync::Arc};

use crate::{Counter, Error, Process};

use super::Sample;

const MAX_COUNTERS: usize = 6;

const KPC_CLASS_FIXED: u32 = 0;
const KPC_CLASS_CONFIGURABLE: u32 = 1;
const KPC_CLASS_POWER: u32 = 2;
const KPC_CLASS_RAWPMU: u32 = 3;

const KPC_CLASS_FIXED_MASK: u32 = 1 << KPC_CLASS_FIXED;
const KPC_CLASS_CONFIGURABLE_MASK: u32 = 1 << KPC_CLASS_CONFIGURABLE;
const KPC_CLASS_POWER_MASK: u32 = 1 << KPC_CLASS_POWER;
const KPC_CLASS_RAWPMU_MASK: u32 = 1 << KPC_CLASS_RAWPMU;

const KPERF_ACTION_MAX: u32 = 32;
const KPERF_TIMER_MAX: u32 = 8;

pub enum CountingDriver {
    InProcess(InProcessDriver),
    Sampling,
}

pub struct SamplingDriver {
    kpc_dispatch: Arc<Container<KPCDispatch>>,
    kpep_dispatch: Arc<Container<KPEPDispatch>>,
    db: *const KPepDB,
    cfg: *mut KPepConfig,
}

pub struct SamplingDriverBuilder {
    counters: Vec<Counter>,
    sample_freq: u64,
    pid: Option<u32>,
}

pub struct InProcessDriver {
    kpc_dispatch: Arc<Container<KPCDispatch>>,
    kpep_dispatch: Arc<Container<KPEPDispatch>>,
    db: *const KPepDB,
    cfg: *mut KPepConfig,
    native_handles: Vec<NativeCounterHandle>,
    counter_values_before: Vec<u64>,
    counter_values_after: Vec<u64>,
    counter_values: Vec<u64>,
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

struct NativeCounterHandle {
    pub kind: Counter,
    pub reg_id: usize,
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
    kpc_get_counter_count: unsafe extern "C" fn(val: u32) -> u32,
    kperf_action_count_set: unsafe extern "C" fn(val: u32) -> c_int,
    kperf_timer_count_set: unsafe extern "C" fn(val: u32) -> c_int,
    kperf_action_samplers_set: unsafe extern "C" fn(action_id: u32, sample: u32) -> c_int,
    kperf_timer_period_set: unsafe extern "C" fn(action_id: u32, tick: u64) -> c_int,
    kperf_timer_action_set: unsafe extern "C" fn(action_id: u32, timer_id: u32) -> c_int,
    kperf_timer_pet_set: unsafe extern "C" fn(timer_id: u32) -> c_int,
    kperf_sample_set: unsafe extern "C" fn(enabled: u32) -> c_int,
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

        let native_handles = counters
            .iter()
            .map(|cntr| NativeCounterHandle {
                kind: cntr.clone(),
                reg_id: 0,
            })
            .collect();

        Ok(CountingDriver::InProcess(InProcessDriver {
            kpc_dispatch: kpc_dispatch.into(),
            kpep_dispatch,
            db,
            cfg,
            native_handles,
            counter_values_before: vec![0; counters.len()],
            counter_values_after: vec![0; counters.len()],
            counter_values: vec![0; counters.len()],
        }))
    }

    pub fn start(&mut self) -> Result<(), Error> {
        match self {
            CountingDriver::InProcess(driver) => driver.start(),
            CountingDriver::Sampling => todo!(),
        }
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        match self {
            CountingDriver::InProcess(driver) => driver.stop(),
            CountingDriver::Sampling => todo!(),
        }
    }
    pub fn reset(&mut self) -> Result<(), Error> {
        match self {
            CountingDriver::InProcess(driver) => driver.reset(),
            CountingDriver::Sampling => todo!(),
        }
    }
    pub fn counters(&mut self) -> Result<CounterResult, Box<dyn std::error::Error>> {
        match self {
            CountingDriver::InProcess(driver) => driver.counters(),
            CountingDriver::Sampling => todo!(),
        }
    }
}

impl InProcessDriver {
    fn start(&mut self) -> Result<(), Error> {
        let mut classes: u32 = 0;
        if unsafe {
            self.kpep_dispatch
                .kpep_config_kpc_classes(self.cfg, &mut classes)
                != 0
        } {
            return Err(Error::EnableFailed);
        }

        let mut reg_count: usize = 0;
        if unsafe {
            self.kpep_dispatch
                .kpep_config_kpc_count(self.cfg, &mut reg_count)
                != 0
        } {
            return Err(Error::EnableFailed);
        }

        let mut native_reg_map = vec![0; 32];
        let ret = unsafe {
            self.kpep_dispatch.kpep_config_kpc_map(
                self.cfg,
                native_reg_map.as_mut_ptr(),
                native_reg_map.len() * std::mem::size_of::<usize>(),
            )
        };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        (0..self.native_handles.len()).for_each(|i| {
            self.native_handles[i].reg_id = native_reg_map[i];
        });

        let mut regs = vec![0; reg_count];
        if unsafe {
            self.kpep_dispatch.kpep_config_kpc(
                self.cfg,
                regs.as_mut_ptr(),
                reg_count * std::mem::size_of::<u64>(),
            ) != 0
        } {
            return Err(Error::EnableFailed);
        }

        if unsafe { self.kpc_dispatch.kpc_force_all_ctrs_set(1) != 0 } {
            return Err(Error::EnableFailed);
        }

        if (classes & KPC_CLASS_CONFIGURABLE_MASK != 0)
            && reg_count != 0
            && unsafe { self.kpc_dispatch.kpc_set_config(classes, regs.as_mut_ptr()) != 0 }
        {
            return Err(Error::EnableFailed);
        }

        if unsafe { self.kpc_dispatch.kpc_set_counting(classes) != 0 } {
            return Err(Error::EnableFailed);
        }
        if unsafe { self.kpc_dispatch.kpc_set_thread_counting(classes) != 0 } {
            return Err(Error::EnableFailed);
        }

        if unsafe {
            self.kpc_dispatch.kpc_get_thread_counters(
                0,
                32,
                self.counter_values_before.as_mut_ptr(),
            ) != 0
        } {
            return Err(Error::EnableFailed);
        }

        Ok(())
    }
    fn stop(&mut self) -> Result<(), Error> {
        if unsafe {
            self.kpc_dispatch
                .kpc_get_thread_counters(0, 32, self.counter_values_after.as_mut_ptr())
                != 0
        } {
            return Err(Error::EnableFailed);
        }
        unsafe {
            self.kpc_dispatch.kpc_set_counting(0);
            self.kpc_dispatch.kpc_set_thread_counting(0);
        }

        for i in 0..self.counter_values.len() {
            self.counter_values[i] += self.counter_values_after[i] - self.counter_values_before[i];
        }

        Ok(())
    }
    fn reset(&mut self) -> Result<(), Error> {
        self.counter_values = vec![0; self.counter_values.len()];
        Ok(())
    }
    fn counters(&mut self) -> Result<CounterResult, Box<dyn std::error::Error>> {
        let values = self
            .native_handles
            .iter()
            .map(|handle| {
                (
                    handle.kind.clone(),
                    CounterValue {
                        value: self.counter_values[handle.reg_id],
                        scaling: 1_f64,
                    },
                )
            })
            .collect();
        Ok(CounterResult { values })
    }
}

impl SamplingDriver {
    pub fn builder() -> SamplingDriverBuilder {
        SamplingDriverBuilder {
            counters: vec![],
            sample_freq: 1000,
            pid: None,
        }
    }

    pub fn start<F>(&self, mut _callback: F) -> Result<(), Error>
    where
        F: FnMut(Sample) + Send + 'static,
    {
        let mut classes: u32 = 0;
        let mut reg_count: usize = 0;
        let mut counter_map = [0_usize; MAX_COUNTERS];
        let mut regs = [0_u64; MAX_COUNTERS];

        let ret = unsafe {
            self.kpep_dispatch
                .kpep_config_kpc_classes(self.cfg, &mut classes)
        };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        let ret = unsafe {
            self.kpep_dispatch
                .kpep_config_kpc_count(self.cfg, &mut reg_count)
        };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        let ret = unsafe {
            self.kpep_dispatch.kpep_config_kpc_map(
                self.cfg,
                counter_map.as_mut_ptr(),
                std::mem::size_of::<[usize; MAX_COUNTERS]>(),
            )
        };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        let ret = unsafe {
            self.kpep_dispatch.kpep_config_kpc(
                self.cfg,
                regs.as_mut_ptr(),
                std::mem::size_of_val(&regs),
            )
        };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        let ret = unsafe { self.kpc_dispatch.kpc_force_all_ctrs_set(1) };
        if ret != 0 {
            return Err(Error::EnableFailed);
        }

        if (classes & KPC_CLASS_CONFIGURABLE_MASK) != 0 && reg_count != 0 {
            let ret = unsafe { self.kpc_dispatch.kpc_set_config(classes, regs.as_mut_ptr()) };
            if ret != 0 {
                return Err(Error::EnableFailed);
            }
        }

        let counter_count = unsafe { self.kpc_dispatch.kpc_get_counter_count(classes) };
        if counter_count == 0 {
            panic!()
        }

        if unsafe { self.kpc_dispatch.kpc_set_counting(classes) != 0 } {
            return Err(Error::EnableFailed);
        }
        if unsafe { self.kpc_dispatch.kpc_set_thread_counting(classes) != 0 } {
            return Err(Error::EnableFailed);
        }

        let action_id = 1_u32;
        let timer_id = 1_u32;

        if unsafe { self.kpc_dispatch.kperf_action_count_set(KPERF_ACTION_MAX) != 0 } {
            return Err(Error::EnableFailed);
        }

        if unsafe { self.kpc_dispatch.kperf_timer_count_set(KPERF_TIMER_MAX) != 0 } {
            return Err(Error::EnableFailed);
        }

        Ok(())
    }

    pub fn stop(&self) -> Result<(), Error> {
        todo!()
    }
}

impl SamplingDriverBuilder {
    pub fn counters(self, counters: &[Counter]) -> Self {
        Self {
            counters: counters.to_vec(),
            sample_freq: self.sample_freq,
            pid: self.pid,
        }
    }

    pub fn pid(self, pid: u32) -> Self {
        Self {
            counters: self.counters,
            sample_freq: self.sample_freq,
            pid: Some(pid),
        }
    }

    pub fn build(self) -> Result<SamplingDriver, Error> {
        todo!()
    }
}

impl CounterResult {
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        self.values
            .iter()
            .find(|(c, _)| *c == kind)
            .map(|(_, v)| v)
            .cloned()
    }
}

impl IntoIterator for CounterResult {
    type Item = (Counter, CounterValue);

    type IntoIter = <Vec<(Counter, CounterValue)> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
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
