use std::sync::Arc;

use dlopen2::wrapper::Container;

use crate::{
    driver::{CounterResult, CounterValue},
    Error,
};

use super::{
    system::{KPCDispatch, KPEPDispatch, KPepConfig, KPepDB, KPC_CLASS_CONFIGURABLE_MASK},
    NativeCounterHandle,
};

pub struct InProcessDriver {
    pub kpc_dispatch: Arc<Container<KPCDispatch>>,
    pub kpep_dispatch: Arc<Container<KPEPDispatch>>,
    pub db: *const KPepDB,
    pub cfg: *mut KPepConfig,
    pub native_handles: Vec<NativeCounterHandle>,
    pub counter_values_before: Vec<u64>,
    pub counter_values_after: Vec<u64>,
    pub counter_values: Vec<u64>,
}

impl InProcessDriver {
    pub fn start(&mut self) -> Result<(), Error> {
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
    pub fn stop(&mut self) -> Result<(), Error> {
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
    pub fn reset(&mut self) -> Result<(), Error> {
        self.counter_values = vec![0; self.counter_values.len()];
        Ok(())
    }
    pub fn counters(&mut self) -> Result<CounterResult, Box<dyn std::error::Error>> {
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
