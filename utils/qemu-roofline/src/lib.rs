use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{c_char, c_int, c_uint, c_void, CStr},
    fs::File,
    io::Write,
    path::PathBuf,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

type PluginId = u64;
type MemInfo = u32;

#[repr(C)]
struct TranslationBlock {
    _private: [u8; 0],
}

#[repr(C)]
struct Instruction {
    _private: [u8; 0],
}

#[repr(C)]
pub struct QemuInfo {
    target_name: *const c_char,
    version: QemuApiVersion,
    system_emulation: bool,
}

#[repr(C)]
struct QemuApiVersion {
    min: c_int,
    current: c_int,
}

#[repr(C)]
struct QemuRegister {
    _private: [u8; 0],
}

#[repr(C)]
struct GArray {
    data: *mut c_char,
    len: c_uint,
}

#[repr(C)]
struct GByteArray {
    data: *mut u8,
    len: c_uint,
}

#[repr(C)]
struct RegisterDescriptor {
    handle: *mut QemuRegister,
    name: *const c_char,
    feature: *const c_char,
    is_readonly: bool,
}

type TranslationCallback = extern "C" fn(PluginId, *mut TranslationBlock);
type ExecutionCallback = extern "C" fn(c_uint, *mut c_void);
type MemoryCallback = extern "C" fn(c_uint, MemInfo, u64, *mut c_void);
type ExitCallback = extern "C" fn(PluginId, *mut c_void);
type VcpuInitCallback = extern "C" fn(PluginId, c_uint);

const CALLBACK_NO_REGS: c_int = 0;
const CALLBACK_READ_REGS: c_int = 1;
const MEMORY_READ_WRITE: c_int = 3;
const REQUIRED_QEMU_PLUGIN_API: c_int = 6;

unsafe extern "C" {
    fn qemu_plugin_register_vcpu_tb_trans_cb(id: PluginId, callback: TranslationCallback);
    fn qemu_plugin_tb_n_insns(tb: *const TranslationBlock) -> usize;
    fn qemu_plugin_tb_vaddr(tb: *const TranslationBlock) -> u64;
    fn qemu_plugin_tb_get_insn(tb: *const TranslationBlock, index: usize) -> *mut Instruction;
    fn qemu_plugin_insn_vaddr(instruction: *const Instruction) -> u64;
    fn qemu_plugin_insn_size(instruction: *const Instruction) -> usize;
    fn qemu_plugin_insn_disas(instruction: *const Instruction) -> *mut c_char;
    fn qemu_plugin_register_vcpu_tb_exec_cb(
        tb: *mut TranslationBlock,
        callback: ExecutionCallback,
        flags: c_int,
        userdata: *mut c_void,
    );
    fn qemu_plugin_register_vcpu_mem_cb(
        instruction: *mut Instruction,
        callback: MemoryCallback,
        flags: c_int,
        read_write: c_int,
        userdata: *mut c_void,
    );
    fn qemu_plugin_register_vcpu_insn_exec_cb(
        instruction: *mut Instruction,
        callback: ExecutionCallback,
        flags: c_int,
        userdata: *mut c_void,
    );
    fn qemu_plugin_register_vcpu_init_cb(id: PluginId, callback: VcpuInitCallback);
    fn qemu_plugin_start_code() -> u64;
    fn qemu_plugin_end_code() -> u64;
    fn qemu_plugin_entry_code() -> u64;
    fn qemu_plugin_get_registers() -> *mut GArray;
    fn qemu_plugin_read_register(handle: *mut QemuRegister, buffer: *mut GByteArray) -> bool;
    fn qemu_plugin_mem_size_shift(info: MemInfo) -> c_uint;
    fn qemu_plugin_mem_is_store(info: MemInfo) -> bool;
    fn qemu_plugin_register_atexit_cb(id: PluginId, callback: ExitCallback, userdata: *mut c_void);
    fn qemu_plugin_outs(message: *const c_char);
    fn g_array_free(array: *mut GArray, free_segment: bool) -> *mut c_char;
    fn g_byte_array_new() -> *mut GByteArray;
    fn g_byte_array_set_size(array: *mut GByteArray, length: c_uint) -> *mut GByteArray;
    fn g_byte_array_unref(array: *mut GByteArray);
    fn g_free(memory: *mut c_void);
}

#[unsafe(no_mangle)]
pub static qemu_plugin_version: c_int = REQUIRED_QEMU_PLUGIN_API;

static OUTPUT: OnceLock<PathBuf> = OnceLock::new();
static TARGET: OnceLock<Target> = OnceLock::new();
static IMAGE: OnceLock<ImageInfo> = OnceLock::new();
static BLOCK_COSTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static RVV_COSTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static RVV_REGISTERS: Mutex<Vec<RvvRegisters>> = Mutex::new(Vec::new());
static SCALAR_INT_OPS: AtomicU64 = AtomicU64::new(0);
static SCALAR_FLOAT_OPS: AtomicU64 = AtomicU64::new(0);
static SCALAR_DOUBLE_OPS: AtomicU64 = AtomicU64::new(0);
static VECTOR_INT_OPS: AtomicU64 = AtomicU64::new(0);
static VECTOR_FLOAT_OPS: AtomicU64 = AtomicU64::new(0);
static VECTOR_DOUBLE_OPS: AtomicU64 = AtomicU64::new(0);
static BYTES_LOAD: AtomicU64 = AtomicU64::new(0);
static BYTES_STORE: AtomicU64 = AtomicU64::new(0);
static DRAM_BYTES_LOAD: AtomicU64 = AtomicU64::new(0);
static DRAM_BYTES_STORE: AtomicU64 = AtomicU64::new(0);
static RVV_STATE_ERRORS: AtomicU64 = AtomicU64::new(0);
static UNCLASSIFIED_INSTRUCTIONS: AtomicU64 = AtomicU64::new(0);
static RVV_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
static UNCLASSIFIED_MNEMONICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CACHE_MODEL: OnceLock<Mutex<CacheModel>> = OnceLock::new();

const DEFAULT_CACHE_LINE_SIZE: u64 = 64;
const DEFAULT_LLC_SIZE: u64 = 8 * 1024 * 1024;
const DEFAULT_LLC_ASSOCIATIVITY: usize = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MemoryTraffic {
    bytes_load: u64,
    bytes_store: u64,
}

#[derive(Clone, Copy, Debug)]
struct CacheLine {
    tag: u64,
    last_used: u64,
    dirty: bool,
}

struct CacheModel {
    line_size: u64,
    capacity: u64,
    associativity: usize,
    sets: Vec<Vec<CacheLine>>,
    clock: u64,
}

impl CacheModel {
    fn new(line_size: u64, capacity: u64, associativity: usize) -> Option<Self> {
        if !line_size.is_power_of_two() || line_size == 0 || associativity == 0 {
            return None;
        }
        let lines = capacity / line_size;
        let set_count = lines / associativity as u64;
        if set_count == 0 {
            return None;
        }
        Some(Self {
            line_size,
            capacity: set_count * associativity as u64 * line_size,
            associativity,
            sets: vec![Vec::with_capacity(associativity); set_count as usize],
            clock: 0,
        })
    }

    fn access(&mut self, address: u64, size: u64, store: bool) -> MemoryTraffic {
        let mut traffic = MemoryTraffic::default();
        if size == 0 {
            return traffic;
        }
        let first_line = address / self.line_size;
        let last_address = address.saturating_add(size - 1);
        let last_line = last_address / self.line_size;

        for line_address in first_line..=last_line {
            self.clock = self.clock.wrapping_add(1);
            let set_index = line_address as usize % self.sets.len();
            let tag = line_address / self.sets.len() as u64;
            let set = &mut self.sets[set_index];
            if let Some(line) = set.iter_mut().find(|line| line.tag == tag) {
                line.last_used = self.clock;
                if store && !line.dirty {
                    line.dirty = true;
                    traffic.bytes_store = traffic.bytes_store.saturating_add(self.line_size);
                }
                continue;
            }

            if set.len() == self.associativity {
                let victim = set
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, line)| line.last_used)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                set.swap_remove(victim);
            }
            set.push(CacheLine {
                tag,
                last_used: self.clock,
                dirty: store,
            });
            if store {
                // Charge a dirty line when it is first created. This models
                // the eventual writeback without assigning a later eviction
                // to an unrelated loop. Read-for-ownership is intentionally
                // excluded to match the conventional Roofline byte model.
                traffic.bytes_store = traffic.bytes_store.saturating_add(self.line_size);
            } else {
                traffic.bytes_load = traffic.bytes_load.saturating_add(self.line_size);
            }
        }
        traffic
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Riscv,
    X86,
}

#[derive(Clone, Copy)]
struct ImageInfo {
    start: u64,
    end: u64,
    entry: u64,
}

#[derive(Clone, Copy, Default)]
struct RvvRegisters {
    vl: usize,
    vtype: usize,
    vstart: usize,
    v0: usize,
}

impl RvvRegisters {
    fn has_required_state(self) -> bool {
        self.vl != 0 && self.vtype != 0
    }
}

struct RegisterBuffer(*mut GByteArray);

impl RegisterBuffer {
    fn new() -> Self {
        Self(unsafe { g_byte_array_new() })
    }
}

impl Drop for RegisterBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { g_byte_array_unref(self.0) };
        }
    }
}

thread_local! {
    static REGISTER_BUFFER: RegisterBuffer = RegisterBuffer::new();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RvvKind {
    Integer,
    Float,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RiscvKind {
    ScalarInteger,
    ScalarFloat,
    ScalarDouble,
    VectorInteger,
    VectorFloat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RiscvCost {
    kind: RiscvKind,
    factor: u64,
    sew_scale: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RiscvClassification {
    Counted(RiscvCost),
    NonCompute,
    Unclassified,
}

struct RiscvOperationSpec {
    mnemonic: &'static str,
    masked: bool,
    classification: RiscvClassification,
}

include!(concat!(env!("OUT_DIR"), "/riscv_operations.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RvvCost {
    block: u64,
    kind: RvvKind,
    factor: u64,
    masked: bool,
    sew_scale: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BlockCost {
    vaddr: u64,
    end_vaddr: u64,
    flow: FlowKind,
    scalar_int: u64,
    scalar_float: u64,
    scalar_double: u64,
    vector_int: u64,
    vector_float: u64,
    vector_double: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FlowKind {
    #[default]
    Normal,
    Call,
    Return,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct DynamicBlockCounts {
    end_vaddr: u64,
    executions: u64,
    scalar_int: u64,
    scalar_float: u64,
    scalar_double: u64,
    vector_int: u64,
    vector_float: u64,
    vector_double: u64,
    bytes_load: u64,
    bytes_store: u64,
    unclassified: u64,
}

struct DynamicCfg {
    last_blocks: Vec<Option<(u64, FlowKind)>>,
    call_stacks: Vec<Vec<u64>>,
    entries: BTreeSet<u64>,
    edges: BTreeMap<(u64, u64), u64>,
    blocks: BTreeMap<u64, DynamicBlockCounts>,
}

static DYNAMIC_CFG: Mutex<DynamicCfg> = Mutex::new(DynamicCfg {
    last_blocks: Vec::new(),
    call_stacks: Vec::new(),
    entries: BTreeSet::new(),
    edges: BTreeMap::new(),
    blocks: BTreeMap::new(),
});

impl BlockCost {
    fn add(&mut self, other: Self) {
        self.scalar_int += other.scalar_int;
        self.scalar_float += other.scalar_float;
        self.scalar_double += other.scalar_double;
        self.vector_int += other.vector_int;
        self.vector_float += other.vector_float;
        self.vector_double += other.vector_double;
    }
}

extern "C" fn execute_block(vcpu: c_uint, userdata: *mut c_void) {
    let cost = unsafe { &*(userdata.cast::<BlockCost>()) };
    SCALAR_INT_OPS.fetch_add(cost.scalar_int, Ordering::Relaxed);
    SCALAR_FLOAT_OPS.fetch_add(cost.scalar_float, Ordering::Relaxed);
    SCALAR_DOUBLE_OPS.fetch_add(cost.scalar_double, Ordering::Relaxed);
    VECTOR_INT_OPS.fetch_add(cost.vector_int, Ordering::Relaxed);
    VECTOR_FLOAT_OPS.fetch_add(cost.vector_float, Ordering::Relaxed);
    VECTOR_DOUBLE_OPS.fetch_add(cost.vector_double, Ordering::Relaxed);

    let mut cfg = DYNAMIC_CFG.lock().unwrap();
    if cfg.last_blocks.len() <= vcpu as usize {
        cfg.last_blocks.resize(vcpu as usize + 1, None);
    }
    if cfg.call_stacks.len() <= vcpu as usize {
        cfg.call_stacks.resize_with(vcpu as usize + 1, Vec::new);
    }
    if let Some((previous, flow)) = cfg.last_blocks[vcpu as usize] {
        match flow {
            FlowKind::Normal => {
                *cfg.edges.entry((previous, cost.vaddr)).or_default() += 1;
            }
            FlowKind::Call => {
                cfg.entries.insert(cost.vaddr);
                cfg.call_stacks[vcpu as usize].push(previous);
            }
            FlowKind::Return => {
                if let Some(caller) = cfg.call_stacks[vcpu as usize].pop() {
                    // Summarize the call as an intraprocedural edge from the
                    // call block to its observed continuation. This preserves
                    // caller loops without adding call/return edges that can
                    // create false interprocedural cycles.
                    *cfg.edges.entry((caller, cost.vaddr)).or_default() += 1;
                } else {
                    cfg.entries.insert(cost.vaddr);
                }
            }
        }
    } else {
        cfg.entries.insert(cost.vaddr);
    }
    cfg.last_blocks[vcpu as usize] = Some((cost.vaddr, cost.flow));
    let block = cfg.blocks.entry(cost.vaddr).or_default();
    if block.end_vaddr == 0 {
        block.end_vaddr = cost.end_vaddr;
    } else if block.end_vaddr != cost.end_vaddr {
        // Self-modifying code or context-dependent translation at the same
        // virtual address cannot be represented as one stable source range.
        block.end_vaddr = block.end_vaddr.max(cost.end_vaddr);
    }
    block.executions = block.executions.saturating_add(1);
    block.scalar_int = block.scalar_int.saturating_add(cost.scalar_int);
    block.scalar_float = block.scalar_float.saturating_add(cost.scalar_float);
    block.scalar_double = block.scalar_double.saturating_add(cost.scalar_double);
    block.vector_int = block.vector_int.saturating_add(cost.vector_int);
    block.vector_float = block.vector_float.saturating_add(cost.vector_float);
    block.vector_double = block.vector_double.saturating_add(cost.vector_double);
}

extern "C" fn execute_unclassified(_vcpu: c_uint, userdata: *mut c_void) {
    UNCLASSIFIED_INSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
    let block_address = userdata as usize as u64;
    let mut cfg = DYNAMIC_CFG.lock().unwrap();
    let block = cfg.blocks.entry(block_address).or_default();
    block.unclassified = block.unclassified.saturating_add(1);
}

extern "C" fn memory_access(_vcpu: c_uint, info: MemInfo, address: u64, userdata: *mut c_void) {
    let shift = unsafe { qemu_plugin_mem_size_shift(info) };
    let bytes = 1_u64.checked_shl(shift).unwrap_or_default();
    let store = unsafe { qemu_plugin_mem_is_store(info) };
    let counter = if store { &BYTES_STORE } else { &BYTES_LOAD };
    counter.fetch_add(bytes, Ordering::Relaxed);
    let traffic = CACHE_MODEL
        .get()
        .expect("cache model initialized before translation")
        .lock()
        .unwrap()
        .access(address, bytes, store);
    DRAM_BYTES_LOAD.fetch_add(traffic.bytes_load, Ordering::Relaxed);
    DRAM_BYTES_STORE.fetch_add(traffic.bytes_store, Ordering::Relaxed);
    let block_address = userdata as usize as u64;
    let mut cfg = DYNAMIC_CFG.lock().unwrap();
    let block = cfg.blocks.entry(block_address).or_default();
    block.bytes_load = block.bytes_load.saturating_add(traffic.bytes_load);
    block.bytes_store = block.bytes_store.saturating_add(traffic.bytes_store);
}

extern "C" fn initialize_vcpu(_id: PluginId, vcpu: c_uint) {
    IMAGE.get_or_init(|| ImageInfo {
        start: unsafe { qemu_plugin_start_code() },
        end: unsafe { qemu_plugin_end_code() },
        entry: unsafe { qemu_plugin_entry_code() },
    });
    if TARGET.get() != Some(&Target::Riscv) {
        return;
    }

    let array = unsafe { qemu_plugin_get_registers() };
    if array.is_null() {
        return;
    }

    let mut registers = RvvRegisters::default();
    let mut register_names = Vec::new();
    let descriptors = unsafe {
        std::slice::from_raw_parts(
            (*array).data.cast::<RegisterDescriptor>(),
            (*array).len as usize,
        )
    };
    for descriptor in descriptors {
        if descriptor.name.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(descriptor.name) }
            .to_string_lossy()
            .to_ascii_lowercase();
        let name = name.trim_start_matches('$');
        register_names.push(name.to_string());
        let handle = descriptor.handle as usize;
        match name {
            "vl" => registers.vl = handle,
            "vtype" => registers.vtype = handle,
            "vstart" => registers.vstart = handle,
            "v0" => registers.v0 = handle,
            _ => {}
        }
    }
    unsafe {
        g_array_free(array, true);
    }

    if !registers.has_required_state() {
        eprintln!(
            "miniperf roofline: RVV registers not exposed by QEMU; available registers: {}",
            register_names.join(", ")
        );
    }
    let mut all_registers = RVV_REGISTERS.lock().unwrap();
    if all_registers.len() <= vcpu as usize {
        all_registers.resize(vcpu as usize + 1, RvvRegisters::default());
    }
    all_registers[vcpu as usize] = registers;
}

extern "C" fn execute_rvv(vcpu: c_uint, userdata: *mut c_void) {
    let cost = unsafe { &*(userdata.cast::<RvvCost>()) };
    let registers = {
        let registers = RVV_REGISTERS.lock().unwrap();
        registers.get(vcpu as usize).copied()
    };
    let Some(registers) = registers.filter(|registers| registers.has_required_state()) else {
        rvv_state_error("register handles are unavailable");
        return;
    };
    let Some((vl, _)) = read_register_value(registers.vl) else {
        rvv_state_error("reading vl failed");
        return;
    };
    let Some((vtype, xlen)) = read_register_value(registers.vtype) else {
        rvv_state_error("reading vtype failed");
        return;
    };
    let vstart = if registers.vstart == 0 {
        0
    } else {
        let Some((vstart, _)) = read_register_value(registers.vstart) else {
            rvv_state_error("reading vstart failed");
            return;
        };
        vstart
    };
    // LMUL and VLEN are already reflected in the architectural vl value. SEW
    // is still needed to select the output precision bucket.
    let Some(sew) = rvv_sew(vtype, xlen).and_then(|sew| sew.checked_mul(cost.sew_scale)) else {
        rvv_state_error("vtype contains vill or an unsupported SEW");
        return;
    };

    let elements = if cost.masked {
        if registers.v0 == 0 {
            rvv_state_error("the v0 mask register is unavailable");
            return;
        }
        let Some(elements) = read_mask_elements(registers.v0, vstart, vl) else {
            rvv_state_error("reading the v0 mask failed");
            return;
        };
        elements
    } else {
        active_elements(vstart, vl, None).unwrap()
    };
    let operations = elements.saturating_mul(cost.factor);
    match cost.kind {
        RvvKind::Integer => {
            VECTOR_INT_OPS.fetch_add(operations, Ordering::Relaxed);
            let mut cfg = DYNAMIC_CFG.lock().unwrap();
            let block = cfg.blocks.entry(cost.block).or_default();
            block.vector_int = block.vector_int.saturating_add(operations);
        }
        RvvKind::Float if sew == 64 => {
            VECTOR_DOUBLE_OPS.fetch_add(operations, Ordering::Relaxed);
            let mut cfg = DYNAMIC_CFG.lock().unwrap();
            let block = cfg.blocks.entry(cost.block).or_default();
            block.vector_double = block.vector_double.saturating_add(operations);
        }
        RvvKind::Float => {
            VECTOR_FLOAT_OPS.fetch_add(operations, Ordering::Relaxed);
            let mut cfg = DYNAMIC_CFG.lock().unwrap();
            let block = cfg.blocks.entry(cost.block).or_default();
            block.vector_float = block.vector_float.saturating_add(operations);
        }
    }
}

fn rvv_state_error(message: &str) {
    RVV_STATE_ERRORS.fetch_add(1, Ordering::Relaxed);
    if !RVV_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("miniperf roofline: {message}");
    }
}

fn read_register_value(handle: usize) -> Option<(u64, u32)> {
    REGISTER_BUFFER.with(|buffer| {
        let buffer = buffer.0;
        if buffer.is_null()
            || unsafe {
                g_byte_array_set_size(buffer, 0);
                !qemu_plugin_read_register(handle as *mut QemuRegister, buffer)
            }
        {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts((*buffer).data, (*buffer).len as usize) };
        if bytes.is_empty() || bytes.len() > size_of::<u64>() {
            return None;
        }
        let mut value = [0_u8; size_of::<u64>()];
        value[..bytes.len()].copy_from_slice(bytes);
        Some((u64::from_le_bytes(value), bytes.len() as u32 * 8))
    })
}

fn read_mask_elements(handle: usize, start: u64, end: u64) -> Option<u64> {
    REGISTER_BUFFER.with(|buffer| {
        let buffer = buffer.0;
        if buffer.is_null()
            || unsafe {
                g_byte_array_set_size(buffer, 0);
                !qemu_plugin_read_register(handle as *mut QemuRegister, buffer)
            }
        {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts((*buffer).data, (*buffer).len as usize) };
        active_elements(start, end, Some(bytes))
    })
}

fn active_elements(start: u64, end: u64, mask: Option<&[u8]>) -> Option<u64> {
    let Some(mask) = mask else {
        return Some(end.saturating_sub(start));
    };
    if end > mask.len() as u64 * 8 {
        return None;
    }
    Some(
        (start..end)
            .filter(|bit| mask[(bit / 8) as usize] & (1 << (bit % 8)) != 0)
            .count() as u64,
    )
}

fn rvv_sew(vtype: u64, xlen: u32) -> Option<u64> {
    if xlen == 0 || xlen > 64 || vtype & (1_u64 << (xlen - 1)) != 0 {
        return None;
    }
    8_u64.checked_shl(((vtype >> 3) & 0x7) as u32)
}

extern "C" fn translate_block(_id: PluginId, tb: *mut TranslationBlock) {
    let block_address = unsafe { qemu_plugin_tb_vaddr(tb) };
    let mut cost = BlockCost {
        vaddr: block_address,
        ..BlockCost::default()
    };
    let instruction_count = unsafe { qemu_plugin_tb_n_insns(tb) };

    for index in 0..instruction_count {
        let instruction = unsafe { qemu_plugin_tb_get_insn(tb, index) };
        if index + 1 == instruction_count {
            cost.end_vaddr = unsafe { qemu_plugin_insn_vaddr(instruction) }
                .saturating_add(unsafe { qemu_plugin_insn_size(instruction) } as u64);
        }
        let disassembly = unsafe { qemu_plugin_insn_disas(instruction) };
        if !disassembly.is_null() {
            let text = unsafe { CStr::from_ptr(disassembly) }.to_string_lossy();
            if index + 1 == instruction_count {
                cost.flow = classify_flow(TARGET.get().copied(), &text);
            }
            if TARGET.get() == Some(&Target::Riscv) {
                match classify_riscv(&text) {
                    RiscvClassification::Counted(riscv_cost) => match riscv_cost.kind {
                        RiscvKind::VectorInteger | RiscvKind::VectorFloat => {
                            let rvv_cost = Box::into_raw(Box::new(RvvCost {
                                block: block_address,
                                kind: if riscv_cost.kind == RiscvKind::VectorFloat {
                                    RvvKind::Float
                                } else {
                                    RvvKind::Integer
                                },
                                factor: riscv_cost.factor,
                                masked: is_masked(&text),
                                sew_scale: riscv_cost.sew_scale,
                            }));
                            RVV_COSTS.lock().unwrap().push(rvv_cost as usize);
                            unsafe {
                                qemu_plugin_register_vcpu_insn_exec_cb(
                                    instruction,
                                    execute_rvv,
                                    CALLBACK_READ_REGS,
                                    rvv_cost.cast(),
                                )
                            };
                        }
                        RiscvKind::ScalarInteger => {
                            cost.scalar_int += riscv_cost.factor;
                        }
                        RiscvKind::ScalarFloat => {
                            cost.scalar_float += riscv_cost.factor;
                        }
                        RiscvKind::ScalarDouble => {
                            cost.scalar_double += riscv_cost.factor;
                        }
                    },
                    RiscvClassification::NonCompute => {}
                    RiscvClassification::Unclassified => {
                        report_unclassified(&text);
                        unsafe {
                            qemu_plugin_register_vcpu_insn_exec_cb(
                                instruction,
                                execute_unclassified,
                                CALLBACK_NO_REGS,
                                block_address as usize as *mut c_void,
                            )
                        };
                    }
                }
            } else {
                cost.add(classify_x86(&mnemonic(&text), &text));
            }
            unsafe { g_free(disassembly.cast()) };
        }
        unsafe {
            qemu_plugin_register_vcpu_mem_cb(
                instruction,
                memory_access,
                CALLBACK_NO_REGS,
                MEMORY_READ_WRITE,
                block_address as usize as *mut c_void,
            )
        };
    }

    let cost = Box::into_raw(Box::new(cost));
    BLOCK_COSTS.lock().unwrap().push(cost as usize);
    unsafe {
        qemu_plugin_register_vcpu_tb_exec_cb(tb, execute_block, CALLBACK_NO_REGS, cost.cast())
    };
}

fn classify_flow(target: Option<Target>, disassembly: &str) -> FlowKind {
    let operation = mnemonic(disassembly);
    let operands = disassembly
        .to_ascii_lowercase()
        .split_once(&operation)
        .map(|(_, operands)| operands.trim().replace(' ', ""))
        .unwrap_or_default();

    match target {
        Some(Target::Riscv) => {
            if matches!(operation.as_str(), "ret" | "c.ret")
                || matches!(operation.as_str(), "jr" | "c.jr")
                    && matches!(operands.as_str(), "ra" | "x1")
                || operation == "jalr"
                    && (operands.starts_with("zero,ra") || operands.starts_with("x0,x1"))
            {
                FlowKind::Return
            } else if operation == "call"
                || matches!(operation.as_str(), "jal" | "jalr")
                    && (operands.starts_with("ra,") || operands.starts_with("x1,"))
                || operation == "c.jal"
                || operation == "c.jalr"
            {
                FlowKind::Call
            } else {
                FlowKind::Normal
            }
        }
        Some(Target::X86) if operation.starts_with("ret") => FlowKind::Return,
        Some(Target::X86) if operation.starts_with("call") => FlowKind::Call,
        _ => FlowKind::Normal,
    }
}

fn mnemonic(disassembly: &str) -> String {
    disassembly
        .split_whitespace()
        .find(|part| {
            part.chars().any(|c| c.is_ascii_alphabetic())
                && !part.trim_end_matches(':').starts_with("0x")
        })
        .unwrap_or_default()
        .trim_matches(|c: char| c == ':' || c == ',')
        .to_ascii_lowercase()
}

fn classify_riscv(disassembly: &str) -> RiscvClassification {
    let mnemonic = mnemonic(disassembly);
    let masked = is_masked(disassembly);
    RISCV_OPERATIONS
        .binary_search_by(|operation| {
            operation
                .mnemonic
                .cmp(mnemonic.as_str())
                .then_with(|| operation.masked.cmp(&masked))
        })
        .ok()
        .map(|index| RISCV_OPERATIONS[index].classification)
        .unwrap_or(RiscvClassification::Unclassified)
}

fn is_masked(disassembly: &str) -> bool {
    disassembly.to_ascii_lowercase().contains("v0.t")
}

fn report_unclassified(disassembly: &str) {
    let mnemonic = mnemonic(disassembly);
    let operation = if is_masked(disassembly) {
        format!("{mnemonic} (masked)")
    } else {
        mnemonic
    };
    let mut reported = UNCLASSIFIED_MNEMONICS.lock().unwrap();
    if !reported.contains(&operation) {
        eprintln!(
            "miniperf roofline: TMDL cannot classify RISC-V operation '{operation}'; counting it as zero"
        );
        reported.push(operation);
    }
}

fn classify_x86(mnemonic: &str, disassembly: &str) -> BlockCost {
    let mut cost = BlockCost::default();
    let opcode = mnemonic.strip_prefix('v').unwrap_or(mnemonic);
    let fused = is_x86_fused(opcode);
    let operations = if fused { 2 } else { 1 };

    if is_x86_float_arithmetic(opcode) {
        if opcode.ends_with("ss") {
            cost.scalar_float = operations;
        } else if opcode.ends_with("sd") {
            cost.scalar_double = operations;
        } else if opcode.ends_with("ps") {
            cost.vector_float = operations * x86_vector_bits(disassembly) / 32;
        } else if opcode.ends_with("pd") {
            cost.vector_double = operations * x86_vector_bits(disassembly) / 64;
        } else if opcode.starts_with('f') {
            // x87 uses extended precision internally. Keep it in the closest
            // existing bucket until the event schema has an explicit type.
            cost.scalar_double = operations;
        }
    } else if let Some(element_bits) = x86_vector_integer_element_bits(opcode) {
        cost.vector_int = operations * x86_vector_bits(disassembly) / element_bits;
    } else if is_x86_scalar_integer(opcode) {
        cost.scalar_int = 1;
    }

    cost
}

fn is_x86_fused(opcode: &str) -> bool {
    ["fmadd", "fmsub", "fnmadd", "fnmsub"]
        .iter()
        .any(|prefix| opcode.starts_with(prefix))
}

fn is_x86_float_arithmetic(opcode: &str) -> bool {
    [
        "add", "sub", "mul", "div", "sqrt", "min", "max", "cmp", "comi", "ucomi", "fmadd", "fmsub",
        "fnmadd", "fnmsub",
    ]
    .iter()
    .any(|prefix| opcode.starts_with(prefix))
        && ["ss", "sd", "ps", "pd"]
            .iter()
            .any(|suffix| opcode.ends_with(suffix))
        || [
            "fadd", "faddp", "fsub", "fsubp", "fsubr", "fmul", "fdiv", "fdivp", "fdivr", "fsqrt",
            "fcom", "fcomp", "fucom", "fucomp",
        ]
        .contains(&opcode)
}

fn x86_vector_bits(disassembly: &str) -> u64 {
    if disassembly.contains("zmm") {
        512
    } else if disassembly.contains("ymm") {
        256
    } else {
        128
    }
}

fn x86_vector_integer_element_bits(opcode: &str) -> Option<u64> {
    let vector_integer = [
        "padd", "psub", "pmul", "pmadd", "pavg", "pmin", "pmax", "pcmpeq", "pcmpgt", "psll",
        "psrl", "psra",
    ]
    .iter()
    .any(|prefix| opcode.starts_with(prefix));
    if !vector_integer {
        return None;
    }

    match opcode.chars().last()? {
        'b' => Some(8),
        'w' => Some(16),
        'd' => Some(32),
        'q' => Some(64),
        _ => None,
    }
}

fn is_x86_scalar_integer(opcode: &str) -> bool {
    let opcodes = [
        "add", "adc", "sub", "sbb", "imul", "mul", "idiv", "div", "inc", "dec", "neg", "shl",
        "shr", "sar", "sal", "and", "or", "xor", "cmp", "test",
    ];
    opcodes.contains(&opcode)
        || opcode
            .strip_suffix(['b', 'w', 'l', 'q'])
            .is_some_and(|opcode| opcodes.contains(&opcode))
}

extern "C" fn plugin_exit(_id: PluginId, _userdata: *mut c_void) {
    if let Some(path) = OUTPUT.get() {
        if let Ok(mut file) = File::create(path) {
            let counters = [
                ("scalar_int_ops", &SCALAR_INT_OPS),
                ("scalar_float_ops", &SCALAR_FLOAT_OPS),
                ("scalar_double_ops", &SCALAR_DOUBLE_OPS),
                ("vector_int_ops", &VECTOR_INT_OPS),
                ("vector_float_ops", &VECTOR_FLOAT_OPS),
                ("vector_double_ops", &VECTOR_DOUBLE_OPS),
                ("bytes_load", &BYTES_LOAD),
                ("bytes_store", &BYTES_STORE),
                ("dram_bytes_load", &DRAM_BYTES_LOAD),
                ("dram_bytes_store", &DRAM_BYTES_STORE),
                ("rvv_state_errors", &RVV_STATE_ERRORS),
                ("unclassified_instructions", &UNCLASSIFIED_INSTRUCTIONS),
            ];
            for (name, counter) in counters {
                let _ = writeln!(file, "{name}={}", counter.load(Ordering::Relaxed));
            }
        }
        let cfg_path = path.with_extension("cfg");
        if let Ok(mut file) = File::create(cfg_path) {
            let cfg = DYNAMIC_CFG.lock().unwrap();
            let _ = writeln!(file, "miniperf-qemu-cfg=3");
            if let Some(cache) = CACHE_MODEL.get() {
                let cache = cache.lock().unwrap();
                let _ = writeln!(
                    file,
                    "cache {} {} {} write-back-no-rfo",
                    cache.line_size, cache.capacity, cache.associativity
                );
            }
            if let Some(image) = IMAGE.get() {
                let _ = writeln!(
                    file,
                    "image {:#x} {:#x} {:#x}",
                    image.start, image.end, image.entry
                );
            }
            for entry in &cfg.entries {
                let _ = writeln!(file, "entry {entry:#x}");
            }
            for (&(from, to), &executions) in &cfg.edges {
                let _ = writeln!(file, "edge {from:#x} {to:#x} {executions}");
            }
            for (&address, counts) in &cfg.blocks {
                let _ = writeln!(
                    file,
                    "block {address:#x} {:#x} {} {} {} {} {} {} {} {} {} {}",
                    counts.end_vaddr,
                    counts.executions,
                    counts.scalar_int,
                    counts.scalar_float,
                    counts.scalar_double,
                    counts.vector_int,
                    counts.vector_float,
                    counts.vector_double,
                    counts.bytes_load,
                    counts.bytes_store,
                    counts.unclassified,
                );
            }
        }
    }

    for cost in std::mem::take(&mut *BLOCK_COSTS.lock().unwrap()) {
        unsafe { drop(Box::from_raw(cost as *mut BlockCost)) };
    }
    for cost in std::mem::take(&mut *RVV_COSTS.lock().unwrap()) {
        unsafe { drop(Box::from_raw(cost as *mut RvvCost)) };
    }
}

/// Installs the plugin into the QEMU process.
///
/// # Safety
///
/// QEMU must pass pointers matching the public `qemu-plugin.h` ABI for the
/// advertised plugin API version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qemu_plugin_install(
    id: PluginId,
    info: *const QemuInfo,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    if info.is_null() {
        return -1;
    }
    if unsafe { (*info).version.current } < REQUIRED_QEMU_PLUGIN_API {
        unsafe { qemu_plugin_outs(c"miniperf roofline: QEMU plugin API 6 is required\n".as_ptr()) };
        return -1;
    }
    let target_name = unsafe { CStr::from_ptr((*info).target_name) }.to_string_lossy();
    let target = if target_name.starts_with("riscv") {
        Target::Riscv
    } else if target_name == "x86_64" || target_name == "i386" {
        Target::X86
    } else {
        unsafe { qemu_plugin_outs(c"miniperf roofline: unsupported QEMU target\n".as_ptr()) };
        return -1;
    };
    if TARGET.set(target).is_err() {
        return -1;
    }
    let args = if argc <= 0 || argv.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(argv, argc as usize) }
    };
    let arguments = args
        .iter()
        .map(|arg| {
            unsafe { CStr::from_ptr(*arg) }
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let output = arguments
        .iter()
        .find_map(|arg| arg.strip_prefix("output=").map(PathBuf::from));
    let Some(output) = output else {
        unsafe { qemu_plugin_outs(c"miniperf roofline: missing output=<path>\n".as_ptr()) };
        return -1;
    };
    if OUTPUT.set(output).is_err() {
        return -1;
    }
    let parse_u64 = |name: &str, default: u64| {
        arguments
            .iter()
            .find_map(|argument| argument.strip_prefix(&format!("{name}=")))
            .map(str::parse::<u64>)
            .transpose()
            .map(|value| value.unwrap_or(default))
    };
    let cache_line = parse_u64("cache-line", DEFAULT_CACHE_LINE_SIZE);
    let cache_size = parse_u64("llc-size", DEFAULT_LLC_SIZE);
    let cache_associativity = parse_u64("llc-assoc", DEFAULT_LLC_ASSOCIATIVITY as u64)
        .ok()
        .and_then(|value| usize::try_from(value).ok());
    let cache = match (cache_line.ok(), cache_size.ok(), cache_associativity) {
        (Some(line), Some(size), Some(associativity)) => CacheModel::new(line, size, associativity),
        _ => None,
    };
    let Some(cache) = cache else {
        unsafe {
            qemu_plugin_outs(
                c"miniperf roofline: invalid cache-line, llc-size, or llc-assoc option\n".as_ptr(),
            )
        };
        return -1;
    };
    if CACHE_MODEL.set(Mutex::new(cache)).is_err() {
        return -1;
    }

    unsafe {
        qemu_plugin_register_vcpu_init_cb(id, initialize_vcpu);
        qemu_plugin_register_vcpu_tb_trans_cb(id, translate_block);
        qemu_plugin_register_atexit_cb(id, plugin_exit, ptr::null_mut());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_model_counts_line_fills_dirty_transitions_and_eviction_reloads() {
        let mut cache = CacheModel::new(64, 128, 1).unwrap();
        assert_eq!(
            cache.access(0, 8, false),
            MemoryTraffic {
                bytes_load: 64,
                bytes_store: 0
            }
        );
        assert_eq!(cache.access(8, 8, false), MemoryTraffic::default());
        assert_eq!(
            cache.access(8, 8, true),
            MemoryTraffic {
                bytes_load: 0,
                bytes_store: 64
            }
        );
        assert_eq!(cache.access(16, 8, true), MemoryTraffic::default());
        assert_eq!(cache.access(128, 8, false).bytes_load, 64);
        assert_eq!(cache.access(0, 8, false).bytes_load, 64);
    }

    #[test]
    fn cache_model_splits_cross_line_accesses() {
        let mut cache = CacheModel::new(64, 256, 1).unwrap();
        assert_eq!(cache.access(60, 8, false).bytes_load, 128);
        assert!(CacheModel::new(48, 256, 1).is_none());
        assert!(CacheModel::new(64, 64, 2).is_none());
    }

    #[test]
    fn classifies_call_and_return_edges() {
        for instruction in [
            "jal ra,0x20",
            "jalr x1,a0,0",
            "call 0x40",
            "c.jal 0x10",
            "c.jalr ra",
        ] {
            assert_eq!(
                classify_flow(Some(Target::Riscv), instruction),
                FlowKind::Call,
                "{instruction}"
            );
        }
        for instruction in ["ret", "jr ra", "jalr zero,ra,0", "c.jr x1"] {
            assert_eq!(
                classify_flow(Some(Target::Riscv), instruction),
                FlowKind::Return,
                "{instruction}"
            );
        }
        assert_eq!(
            classify_flow(Some(Target::Riscv), "jal zero,0x20"),
            FlowKind::Normal
        );
        assert_eq!(
            classify_flow(Some(Target::X86), "callq 0x20"),
            FlowKind::Call
        );
        assert_eq!(classify_flow(Some(Target::X86), "retq"), FlowKind::Return);
    }

    #[test]
    fn classifies_tmdl_scalar_and_compressed_operations() {
        assert_eq!(
            classify_riscv("fadd.d fa0,fa1,fa2"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::ScalarDouble,
                factor: 1,
                sew_scale: 1,
            })
        );
        assert_eq!(
            classify_riscv("c.add a0,a1"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::ScalarInteger,
                factor: 1,
                sew_scale: 1,
            })
        );
    }

    #[test]
    fn classifies_tmdl_vector_arithmetic() {
        assert_eq!(
            classify_riscv("vfmacc.vv v8,v9,v10,v0.t"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::VectorFloat,
                factor: 2,
                sew_scale: 1,
            })
        );
        assert_eq!(
            classify_riscv("vfwadd.vv v8,v9,v10"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::VectorFloat,
                factor: 1,
                sew_scale: 2,
            })
        );
        assert_eq!(
            classify_riscv("vfredusum.vs v8,v9,v10"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::VectorFloat,
                factor: 1,
                sew_scale: 1,
            })
        );
        assert_eq!(
            classify_riscv("vmacc.vv v8,v9,v10"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::VectorInteger,
                factor: 2,
                sew_scale: 1,
            })
        );
    }

    #[test]
    fn does_not_count_control_flow_or_expand_integer_remainder() {
        for instruction in ["auipc a0,0x10", "jal ra,0x20", "jalr ra,a0,0"] {
            assert_eq!(
                classify_riscv(instruction),
                RiscvClassification::NonCompute,
                "{instruction}"
            );
        }
        assert_eq!(
            classify_riscv("rem a0,a1,a2"),
            RiscvClassification::Counted(RiscvCost {
                kind: RiscvKind::ScalarInteger,
                factor: 1,
                sew_scale: 1,
            })
        );
    }

    #[test]
    fn treats_non_arithmetic_float_behavior_as_non_compute() {
        for instruction in [
            "vmfeq.vv v1,v2,v3",
            "vfcvt.x.f.v v1,v2",
            "vfsgnj.vv v1,v2,v3",
        ] {
            assert_eq!(
                classify_riscv(instruction),
                RiscvClassification::NonCompute,
                "{instruction}"
            );
        }
    }

    #[test]
    fn leaves_missing_and_todo_operations_unclassified() {
        assert_eq!(
            classify_riscv("vfrsqrt7.v v1,v2"),
            RiscvClassification::Unclassified
        );
        assert_eq!(
            classify_riscv("madeup a0,a1"),
            RiscvClassification::Unclassified
        );

        let before = UNCLASSIFIED_INSTRUCTIONS.load(Ordering::Relaxed);
        execute_unclassified(0, ptr::null_mut());
        assert_eq!(
            UNCLASSIFIED_INSTRUCTIONS.load(Ordering::Relaxed),
            before + 1
        );
    }

    #[test]
    fn applies_rvv_mask_vstart_and_sew() {
        assert_eq!(active_elements(2, 7, Some(&[0b0110_1100])), Some(4));
        assert_eq!(active_elements(2, 7, None), Some(5));
        assert_eq!(active_elements(9, 7, None), Some(0));
        assert_eq!(active_elements(0, 9, Some(&[0xff])), None);
        assert_eq!(rvv_sew(2 << 3, 64), Some(32));
        assert_eq!(rvv_sew(3 << 3, 64), Some(64));
        assert_eq!(rvv_sew(1_u64 << 63, 64), None);
    }

    #[test]
    fn classifies_x86_scalar_and_vector_operations() {
        let scalar = classify_x86("mulsd", "mulsd 0x8(%rax),%xmm0");
        assert_eq!(scalar.scalar_double, 1);

        let vector = classify_x86("vfmadd132ps", "vfmadd132ps %ymm1,%ymm2,%ymm3");
        assert_eq!(vector.vector_float, 16);

        let integer = classify_x86("vpaddd", "vpaddd %zmm1,%zmm2,%zmm3");
        assert_eq!(integer.vector_int, 16);
    }

    #[test]
    fn does_not_treat_x86_simd_logic_as_scalar_integer_work() {
        let cost = classify_x86("orpd", "orpd %xmm1,%xmm0");
        assert_eq!(cost.scalar_int, 0);
    }
}
