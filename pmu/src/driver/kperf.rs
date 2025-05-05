#![allow(dead_code)]

mod inprocess;
mod system;

use dlopen2::wrapper::{Container, WrapperApi};
use inprocess::InProcessDriver;
use libc::*;
use std::{ffi::CStr, sync::Arc};
use system::{
    KPCDispatch, KPEPDispatch, KPepConfig, KPepDB, KPepEvent, KPC_CLASS_CONFIGURABLE_MASK,
    KPERF_ACTION_MAX, KPERF_TIMER_MAX,
};

use crate::{Counter, Error, Process};

use super::{CounterResult, CountingDriver, Sample};

const MAX_COUNTERS: usize = 6;

pub enum KPerfCountingDriver {
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

pub struct NativeCounterHandle {
    pub kind: Counter,
    pub reg_id: usize,
}

impl KPerfCountingDriver {
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

        Ok(KPerfCountingDriver::InProcess(InProcessDriver {
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
}

impl CountingDriver for KPerfCountingDriver {
    fn start(&mut self) -> Result<(), Error> {
        match self {
            KPerfCountingDriver::InProcess(driver) => driver.start(),
            KPerfCountingDriver::Sampling => todo!(),
        }
    }
    fn stop(&mut self) -> Result<(), Error> {
        match self {
            KPerfCountingDriver::InProcess(driver) => driver.stop(),
            KPerfCountingDriver::Sampling => todo!(),
        }
    }
    fn reset(&mut self) -> Result<(), Error> {
        match self {
            KPerfCountingDriver::InProcess(driver) => driver.reset(),
            KPerfCountingDriver::Sampling => todo!(),
        }
    }
    fn counters(&mut self) -> Result<CounterResult, std::io::Error> {
        match self {
            KPerfCountingDriver::InProcess(driver) => Ok(driver.counters().expect("")),
            KPerfCountingDriver::Sampling => todo!(),
        }
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
