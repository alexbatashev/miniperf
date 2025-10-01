use libc::*;

use dlopen2::wrapper::{Container, WrapperApi};

pub const KPC_CLASS_FIXED: u32 = 0;
pub const KPC_CLASS_CONFIGURABLE: u32 = 1;
pub const KPC_CLASS_POWER: u32 = 2;
pub const KPC_CLASS_RAWPMU: u32 = 3;

pub const KPC_CLASS_FIXED_MASK: u32 = 1 << KPC_CLASS_FIXED;
pub const KPC_CLASS_CONFIGURABLE_MASK: u32 = 1 << KPC_CLASS_CONFIGURABLE;
pub const KPC_CLASS_POWER_MASK: u32 = 1 << KPC_CLASS_POWER;
pub const KPC_CLASS_RAWPMU_MASK: u32 = 1 << KPC_CLASS_RAWPMU;

pub const KPERF_ACTION_MAX: u32 = 32;
pub const KPERF_TIMER_MAX: u32 = 8;

#[repr(C)]
pub struct KPepEvent {
    pub name: *const c_char,
    pub description: *const c_char,
    pub errata: *const c_char,
    pub alias: *const c_char,
    pub fallback: *const c_char,
    pub mask: u32,
    pub number: u8,
    pub umask: u8,
    pub reserved: u8,
    pub is_fixed: u8,
}

#[derive(WrapperApi)]
pub struct KPCDispatch {
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

#[repr(C)]
pub struct KPepDB {
    pub name: *const c_char,
    pub cpu_id: *const c_char,
    pub marketing_name: *const c_char,
    pub plist_data: *const u8,
    pub event_map: *const u8,
    pub event_arr: *const KPepEvent,
    pub fixed_event_arr: *const *const KPepEvent,
    pub alias_map: *const u8,
    pub reserved1: size_t,
    pub reserved2: size_t,
    pub reserved3: size_t,
    pub event_count: size_t,
    pub alias_count: size_t,
    pub fixed_count_counter: size_t,
    pub config_counter_count: size_t,
    pub power_counter_count: size_t,
    pub architecture: u32,
    pub fixed_counter_bits: u32,
    pub config_counter_bits: u32,
    pub power_counter_bits: u32,
}

#[repr(C)]
pub struct KPepConfig {
    pub db: *const KPepDB,
    pub ev_arr: *const *const KPepEvent,
    pub ev_map: *const size_t,
    pub ev_idx: *const size_t,
    pub flags: *const u32,
    pub kpc_periods: *const u64,
    pub event_count: size_t,
    pub counter_count: size_t,
    pub classes: u32,
    pub config_counter: u32,
    pub power_counter: u32,
    pub reserved: u32,
}

#[derive(WrapperApi)]
pub struct KPEPDispatch {
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
