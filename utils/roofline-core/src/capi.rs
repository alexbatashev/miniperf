//! C ABI for the DynamoRIO client. The client's C glue forwards
//! instrumentation events here; all accounting and artifact writing is shared
//! with the QEMU plugin.
//!
//! Mirrored by `utils/dr-roofline/roofline_core.h` — keep in sync.

use crate::artifacts::{self, CacheDescription, CounterSnapshot, ImageInfo};
use crate::cache::CacheModel;
use crate::cfg::{DynamicCfg, VectorClass};
use crate::classify::{self, BlockCost, FlowKind, RiscvClassification, RiscvKind, Target};
use crate::memory::MemoryAnalysis;
use std::ffi::{c_char, CStr};
use std::path::PathBuf;
use std::sync::Mutex;

pub const RC_TARGET_X86: u32 = 0;
pub const RC_TARGET_RISCV: u32 = 1;
pub const RC_TARGET_AARCH64: u32 = 2;

pub const RC_KIND_NONE: u32 = 0;
pub const RC_KIND_STATIC: u32 = 1;
pub const RC_KIND_RVV: u32 = 2;
pub const RC_KIND_UNCLASSIFIED: u32 = 3;

pub const RC_FLOW_NORMAL: u32 = 0;
pub const RC_FLOW_CALL: u32 = 1;
pub const RC_FLOW_RETURN: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RcCost {
    pub scalar_int: u64,
    pub scalar_float: u64,
    pub scalar_double: u64,
    pub vector_int: u64,
    pub vector_float: u64,
    pub vector_double: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RcClassification {
    pub kind: u32,
    pub cost: RcCost,
    pub rvv_is_float: u32,
    pub rvv_masked: u32,
    pub rvv_factor: u64,
    pub rvv_sew_scale: u64,
}

struct Inner {
    counters: CounterSnapshot,
    cfg: DynamicCfg,
    cache: CacheModel,
    memory: Option<MemoryAnalysis>,
    image: Option<ImageInfo>,
    output: PathBuf,
}

pub struct Session {
    inner: Mutex<Inner>,
}

fn target_from(raw: u32) -> Option<Target> {
    match raw {
        RC_TARGET_X86 => Some(Target::X86),
        RC_TARGET_RISCV => Some(Target::Riscv),
        RC_TARGET_AARCH64 => Some(Target::Aarch64),
        _ => None,
    }
}

/// # Safety
/// `output` must be a valid NUL-terminated path string.
#[no_mangle]
pub unsafe extern "C" fn rc_session_new(
    output: *const c_char,
    cache_line: u64,
    llc_size: u64,
    llc_associativity: u64,
    memory_profile: u32,
) -> *mut Session {
    if output.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(associativity) = usize::try_from(llc_associativity) else {
        return std::ptr::null_mut();
    };
    let Some(cache) = CacheModel::new(cache_line, llc_size, associativity) else {
        return std::ptr::null_mut();
    };
    let path = PathBuf::from(
        unsafe { CStr::from_ptr(output) }
            .to_string_lossy()
            .into_owned(),
    );
    let line_size = cache.line_size();
    let session = Session {
        inner: Mutex::new(Inner {
            counters: CounterSnapshot::default(),
            cfg: DynamicCfg::new(),
            cache,
            memory: (memory_profile != 0).then(|| MemoryAnalysis::new(line_size)),
            image: None,
            output: path,
        }),
    };
    Box::into_raw(Box::new(session))
}

/// # Safety
/// `session` must be a live pointer from `rc_session_new`.
#[no_mangle]
pub unsafe extern "C" fn rc_session_set_image(
    session: *mut Session,
    start: u64,
    end: u64,
    entry: u64,
) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    session.inner.lock().unwrap().image = Some(ImageInfo { start, end, entry });
}

/// Classifies one instruction from its disassembly text.
///
/// # Safety
/// `disassembly` must be a valid NUL-terminated string and `out` a valid
/// pointer.
#[no_mangle]
pub unsafe extern "C" fn rc_classify(
    target: u32,
    disassembly: *const c_char,
    out: *mut RcClassification,
) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    *out = RcClassification::default();
    if disassembly.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(disassembly) }.to_string_lossy();
    match target_from(target) {
        Some(Target::Riscv) => match classify::classify_riscv(&text) {
            RiscvClassification::Counted(cost) => match cost.kind {
                RiscvKind::VectorInteger | RiscvKind::VectorFloat => {
                    out.kind = RC_KIND_RVV;
                    out.rvv_is_float = u32::from(cost.kind == RiscvKind::VectorFloat);
                    out.rvv_masked = u32::from(classify::is_masked(&text));
                    out.rvv_factor = cost.factor;
                    out.rvv_sew_scale = cost.sew_scale;
                }
                RiscvKind::ScalarInteger => {
                    out.kind = RC_KIND_STATIC;
                    out.cost.scalar_int = cost.factor;
                }
                RiscvKind::ScalarFloat => {
                    out.kind = RC_KIND_STATIC;
                    out.cost.scalar_float = cost.factor;
                }
                RiscvKind::ScalarDouble => {
                    out.kind = RC_KIND_STATIC;
                    out.cost.scalar_double = cost.factor;
                }
            },
            RiscvClassification::NonCompute => out.kind = RC_KIND_NONE,
            RiscvClassification::Unclassified => out.kind = RC_KIND_UNCLASSIFIED,
        },
        Some(Target::X86) => {
            let cost = classify::classify_x86(&classify::mnemonic(&text), &text);
            fill_static(out, &cost);
        }
        Some(Target::Aarch64) => {
            let cost = classify::classify_aarch64(&classify::mnemonic(&text), &text);
            fill_static(out, &cost);
        }
        None => {}
    }
}

fn fill_static(out: &mut RcClassification, cost: &BlockCost) {
    out.kind = RC_KIND_STATIC;
    out.cost = RcCost {
        scalar_int: cost.scalar_int,
        scalar_float: cost.scalar_float,
        scalar_double: cost.scalar_double,
        vector_int: cost.vector_int,
        vector_float: cost.vector_float,
        vector_double: cost.vector_double,
    };
}

/// # Safety
/// `disassembly` must be a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn rc_flow_kind(target: u32, disassembly: *const c_char) -> u32 {
    if disassembly.is_null() {
        return RC_FLOW_NORMAL;
    }
    let text = unsafe { CStr::from_ptr(disassembly) }.to_string_lossy();
    match classify::classify_flow(target_from(target), &text) {
        FlowKind::Normal => RC_FLOW_NORMAL,
        FlowKind::Call => RC_FLOW_CALL,
        FlowKind::Return => RC_FLOW_RETURN,
    }
}

/// Records one execution of a block.
///
/// # Safety
/// `session` and `cost` must be valid pointers.
#[no_mangle]
pub unsafe extern "C" fn rc_block_exec(
    session: *mut Session,
    thread: u32,
    vaddr: u64,
    end_vaddr: u64,
    flow: u32,
    cost: *const RcCost,
    instructions: u64,
) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    let Some(cost) = (unsafe { cost.as_ref() }) else {
        return;
    };
    let block = BlockCost {
        vaddr,
        end_vaddr,
        flow: match flow {
            RC_FLOW_CALL => FlowKind::Call,
            RC_FLOW_RETURN => FlowKind::Return,
            _ => FlowKind::Normal,
        },
        scalar_int: cost.scalar_int,
        scalar_float: cost.scalar_float,
        scalar_double: cost.scalar_double,
        vector_int: cost.vector_int,
        vector_float: cost.vector_float,
        vector_double: cost.vector_double,
        instructions,
    };
    let mut inner = session.inner.lock().unwrap();
    let counters = &mut inner.counters;
    counters.instructions = counters.instructions.saturating_add(instructions);
    counters.scalar_int_ops = counters.scalar_int_ops.saturating_add(cost.scalar_int);
    counters.scalar_float_ops = counters.scalar_float_ops.saturating_add(cost.scalar_float);
    counters.scalar_double_ops = counters
        .scalar_double_ops
        .saturating_add(cost.scalar_double);
    counters.vector_int_ops = counters.vector_int_ops.saturating_add(cost.vector_int);
    counters.vector_float_ops = counters.vector_float_ops.saturating_add(cost.vector_float);
    counters.vector_double_ops = counters
        .vector_double_ops
        .saturating_add(cost.vector_double);
    inner.cfg.record_block(thread as usize, &block);
}

/// Records one memory access issued by `block`.
///
/// # Safety
/// `session` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn rc_mem_access(
    session: *mut Session,
    thread: u32,
    block: u64,
    address: u64,
    size: u64,
    is_store: u32,
) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    let store = is_store != 0;
    let mut inner = session.inner.lock().unwrap();
    if let Some(memory) = inner.memory.as_mut() {
        memory.access(thread as usize, address, size, store);
    }
    if store {
        inner.counters.bytes_store = inner.counters.bytes_store.saturating_add(size);
    } else {
        inner.counters.bytes_load = inner.counters.bytes_load.saturating_add(size);
    }
    let traffic = inner.cache.access(address, size, store);
    inner.counters.dram_bytes_load = inner
        .counters
        .dram_bytes_load
        .saturating_add(traffic.bytes_load);
    inner.counters.dram_bytes_store = inner
        .counters
        .dram_bytes_store
        .saturating_add(traffic.bytes_store);
    inner
        .cfg
        .attribute_memory(block, traffic.bytes_load, traffic.bytes_store);
}

/// Records dynamically-counted RVV operations for `block`.
///
/// # Safety
/// `session` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn rc_rvv_exec(
    session: *mut Session,
    block: u64,
    is_float: u32,
    sew_bits: u64,
    operations: u64,
) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    let mut inner = session.inner.lock().unwrap();
    let class = if is_float == 0 {
        inner.counters.vector_int_ops = inner.counters.vector_int_ops.saturating_add(operations);
        VectorClass::Integer
    } else if sew_bits == 64 {
        inner.counters.vector_double_ops =
            inner.counters.vector_double_ops.saturating_add(operations);
        VectorClass::Double
    } else {
        inner.counters.vector_float_ops =
            inner.counters.vector_float_ops.saturating_add(operations);
        VectorClass::Float
    };
    inner.cfg.attribute_vector(block, class, operations);
}

/// # Safety
/// `session` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn rc_unclassified(session: *mut Session, block: u64) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    let mut inner = session.inner.lock().unwrap();
    inner.counters.unclassified_instructions =
        inner.counters.unclassified_instructions.saturating_add(1);
    inner.cfg.attribute_unclassified(block);
}

/// # Safety
/// `session` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn rc_rvv_state_error(session: *mut Session) {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return;
    };
    let mut inner = session.inner.lock().unwrap();
    inner.counters.rvv_state_errors = inner.counters.rvv_state_errors.saturating_add(1);
}

/// Counts active elements in [vstart, vl) under an optional v0 mask.
/// Returns -1 when the mask is too short for vl.
///
/// # Safety
/// `mask` must point to `mask_len` readable bytes when non-null.
#[no_mangle]
pub unsafe extern "C" fn rc_active_elements(
    vstart: u64,
    vl: u64,
    mask: *const u8,
    mask_len: u64,
) -> i64 {
    let mask = if mask.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(mask, mask_len as usize) })
    };
    match classify::active_elements(vstart, vl, mask) {
        Some(elements) => elements as i64,
        None => -1,
    }
}

/// Decodes SEW bits from vtype. Returns -1 when vtype is invalid (vill).
#[no_mangle]
pub extern "C" fn rc_rvv_sew(vtype: u64, xlen: u32) -> i64 {
    match classify::rvv_sew(vtype, xlen) {
        Some(sew) => sew as i64,
        None => -1,
    }
}

/// Writes the three artifacts and frees the session. `session` is invalid
/// afterwards.
///
/// # Safety
/// `session` must be a live pointer from `rc_session_new`, not used again.
#[no_mangle]
pub unsafe extern "C" fn rc_finalize(session: *mut Session) -> i32 {
    if session.is_null() {
        return -1;
    }
    let session = unsafe { Box::from_raw(session) };
    let mut inner = session.inner.lock().unwrap();
    let flush = inner.cache.flush();
    inner.counters.dram_bytes_load = inner
        .counters
        .dram_bytes_load
        .saturating_add(flush.bytes_load);
    inner.counters.dram_bytes_store = inner
        .counters
        .dram_bytes_store
        .saturating_add(flush.bytes_store);

    let output = inner.output.clone();
    artifacts::write_counts(&output, &inner.counters);
    artifacts::write_cfg(
        &output.with_extension("cfg"),
        Some(CacheDescription {
            line_size: inner.cache.line_size(),
            capacity: inner.cache.capacity(),
            associativity: inner.cache.associativity(),
        }),
        inner.image,
        &inner.cfg,
    );
    if let Some(memory) = inner.memory.as_ref() {
        artifacts::write_memory(&output.with_extension("memory.json"), &memory.artifact());
    }
    0
}
