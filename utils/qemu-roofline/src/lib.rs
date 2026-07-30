use std::{
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
    fn qemu_plugin_tb_get_insn(tb: *const TranslationBlock, index: usize) -> *mut Instruction;
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
static RVV_STATE_ERRORS: AtomicU64 = AtomicU64::new(0);
static RVV_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Riscv,
    X86,
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
struct RvvCost {
    kind: RvvKind,
    factor: u64,
    masked: bool,
    sew_scale: u64,
}

#[derive(Default)]
struct BlockCost {
    scalar_int: u64,
    scalar_float: u64,
    scalar_double: u64,
    vector_int: u64,
    vector_float: u64,
    vector_double: u64,
}

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

extern "C" fn execute_block(_vcpu: c_uint, userdata: *mut c_void) {
    let cost = unsafe { &*(userdata.cast::<BlockCost>()) };
    SCALAR_INT_OPS.fetch_add(cost.scalar_int, Ordering::Relaxed);
    SCALAR_FLOAT_OPS.fetch_add(cost.scalar_float, Ordering::Relaxed);
    SCALAR_DOUBLE_OPS.fetch_add(cost.scalar_double, Ordering::Relaxed);
    VECTOR_INT_OPS.fetch_add(cost.vector_int, Ordering::Relaxed);
    VECTOR_FLOAT_OPS.fetch_add(cost.vector_float, Ordering::Relaxed);
    VECTOR_DOUBLE_OPS.fetch_add(cost.vector_double, Ordering::Relaxed);
}

extern "C" fn memory_access(_vcpu: c_uint, info: MemInfo, _address: u64, _userdata: *mut c_void) {
    let shift = unsafe { qemu_plugin_mem_size_shift(info) };
    let bytes = 1_u64.checked_shl(shift).unwrap_or_default();
    let counter = if unsafe { qemu_plugin_mem_is_store(info) } {
        &BYTES_STORE
    } else {
        &BYTES_LOAD
    };
    counter.fetch_add(bytes, Ordering::Relaxed);
}

extern "C" fn initialize_vcpu(_id: PluginId, vcpu: c_uint) {
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
        vl.saturating_sub(vstart)
    };
    let operations = elements.saturating_mul(cost.factor);
    match cost.kind {
        RvvKind::Integer => {
            VECTOR_INT_OPS.fetch_add(operations, Ordering::Relaxed);
        }
        RvvKind::Float if sew == 64 => {
            VECTOR_DOUBLE_OPS.fetch_add(operations, Ordering::Relaxed);
        }
        RvvKind::Float => {
            VECTOR_FLOAT_OPS.fetch_add(operations, Ordering::Relaxed);
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
    if start >= end {
        return Some(0);
    }
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
        if end > bytes.len() as u64 * 8 {
            return None;
        }
        Some(
            (start..end)
                .filter(|bit| bytes[(bit / 8) as usize] & (1 << (bit % 8)) != 0)
                .count() as u64,
        )
    })
}

fn rvv_sew(vtype: u64, xlen: u32) -> Option<u64> {
    if xlen == 0 || xlen > 64 || vtype & (1_u64 << (xlen - 1)) != 0 {
        return None;
    }
    8_u64.checked_shl(((vtype >> 3) & 0x7) as u32)
}

extern "C" fn translate_block(_id: PluginId, tb: *mut TranslationBlock) {
    let mut cost = BlockCost::default();
    let instruction_count = unsafe { qemu_plugin_tb_n_insns(tb) };

    for index in 0..instruction_count {
        let instruction = unsafe { qemu_plugin_tb_get_insn(tb, index) };
        let disassembly = unsafe { qemu_plugin_insn_disas(instruction) };
        if !disassembly.is_null() {
            let text = unsafe { CStr::from_ptr(disassembly) }.to_string_lossy();
            if TARGET.get() == Some(&Target::Riscv) {
                if let Some(rvv_cost) = classify_rvv(&text) {
                    let rvv_cost = Box::into_raw(Box::new(rvv_cost));
                    RVV_COSTS.lock().unwrap().push(rvv_cost as usize);
                    unsafe {
                        qemu_plugin_register_vcpu_insn_exec_cb(
                            instruction,
                            execute_rvv,
                            CALLBACK_READ_REGS,
                            rvv_cost.cast(),
                        )
                    };
                } else {
                    cost.add(classify_riscv(&mnemonic(&text)));
                }
            } else {
                cost.add(classify(Target::X86, &text));
            }
            unsafe { g_free(disassembly.cast()) };
        }
        unsafe {
            qemu_plugin_register_vcpu_mem_cb(
                instruction,
                memory_access,
                CALLBACK_NO_REGS,
                MEMORY_READ_WRITE,
                ptr::null_mut(),
            )
        };
    }

    let cost = Box::into_raw(Box::new(cost));
    BLOCK_COSTS.lock().unwrap().push(cost as usize);
    unsafe {
        qemu_plugin_register_vcpu_tb_exec_cb(tb, execute_block, CALLBACK_NO_REGS, cost.cast())
    };
}

fn classify(target: Target, disassembly: &str) -> BlockCost {
    let mnemonic = mnemonic(disassembly);
    match target {
        Target::Riscv => classify_riscv(&mnemonic),
        Target::X86 => classify_x86(&mnemonic, disassembly),
    }
}

fn mnemonic(disassembly: &str) -> String {
    let mnemonic = disassembly
        .split_whitespace()
        .find(|part| {
            part.chars().any(|c| c.is_ascii_alphabetic())
                && !part.trim_end_matches(':').starts_with("0x")
        })
        .unwrap_or_default()
        .trim_matches(|c: char| c == ':' || c == ',')
        .to_ascii_lowercase();
    mnemonic.strip_prefix("c.").unwrap_or(&mnemonic).to_string()
}

fn classify_riscv(mnemonic: &str) -> BlockCost {
    let mut cost = BlockCost::default();

    if is_riscv_scalar_float(mnemonic) {
        let operations = if is_fused(mnemonic) { 2 } else { 1 };
        if mnemonic.ends_with(".d") {
            cost.scalar_double = operations;
        } else {
            cost.scalar_float = operations;
        }
    } else if is_riscv_scalar_integer(mnemonic) {
        cost.scalar_int = 1;
    }

    cost
}

fn classify_rvv(disassembly: &str) -> Option<RvvCost> {
    let mnemonic = mnemonic(disassembly);
    let kind = if is_riscv_vector_float(&mnemonic) {
        RvvKind::Float
    } else if is_riscv_vector_integer(&mnemonic) {
        RvvKind::Integer
    } else {
        return None;
    };
    let factor = if is_fused(&mnemonic)
        || (kind == RvvKind::Integer
            && ["macc", "madd", "msac", "msub"]
                .iter()
                .any(|part| mnemonic.contains(part)))
    {
        2
    } else {
        1
    };
    Some(RvvCost {
        kind,
        factor,
        masked: disassembly.to_ascii_lowercase().contains("v0.t"),
        sew_scale: if mnemonic.starts_with("vfw") { 2 } else { 1 },
    })
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

fn is_fused(mnemonic: &str) -> bool {
    mnemonic.starts_with("fmadd")
        || mnemonic.starts_with("fmsub")
        || mnemonic.starts_with("fnmadd")
        || mnemonic.starts_with("fnmsub")
        || mnemonic.starts_with("vfmacc")
        || mnemonic.starts_with("vfmsac")
        || mnemonic.starts_with("vfnmacc")
        || mnemonic.starts_with("vfnmsac")
        || mnemonic.starts_with("vfmadd")
        || mnemonic.starts_with("vfmsub")
        || mnemonic.starts_with("vfnmadd")
        || mnemonic.starts_with("vfnmsub")
}

fn is_riscv_scalar_float(mnemonic: &str) -> bool {
    [
        "fadd", "fsub", "fmul", "fdiv", "fsqrt", "fmin", "fmax", "feq", "flt", "fle", "fmadd",
        "fmsub", "fnmadd", "fnmsub",
    ]
    .iter()
    .any(|prefix| mnemonic.starts_with(prefix))
}

fn is_riscv_vector_float(mnemonic: &str) -> bool {
    [
        "vfadd", "vfsub", "vfrsub", "vfmul", "vfdiv", "vfrdiv", "vfsqrt", "vfmin", "vfmax",
        "vmfeq", "vmfne", "vmflt", "vmfle", "vmfgt", "vmfge", "vfmacc", "vfmsac", "vfnmacc",
        "vfnmsac", "vfmadd", "vfmsub", "vfnmadd", "vfnmsub", "vfwadd", "vfwsub", "vfwmul",
        "vfwmacc", "vfwmsac", "vfwnmacc", "vfwnmsac",
    ]
    .iter()
    .any(|prefix| mnemonic.starts_with(prefix))
}

fn is_riscv_vector_integer(mnemonic: &str) -> bool {
    [
        "vadd", "vsub", "vrsub", "vmul", "vdiv", "vrem", "vsll", "vsrl", "vsra", "vand", "vor",
        "vxor", "vmin", "vmax", "vmseq", "vmsne", "vmslt", "vmsle", "vmsgt", "vmacc", "vnmsac",
        "vmadd", "vnmsub", "vwadd", "vwsub", "vwmul", "vwmacc", "vwmadd", "vwnmsac", "vwnmsub",
    ]
    .iter()
    .any(|prefix| mnemonic.starts_with(prefix))
}

fn is_riscv_scalar_integer(mnemonic: &str) -> bool {
    [
        "add", "sub", "mul", "div", "rem", "sll", "srl", "sra", "and", "or", "xor", "slt", "min",
        "max", "sh1add", "sh2add", "sh3add", "amoadd", "amoand", "amoor", "amoxor", "amomin",
        "amomax",
    ]
    .iter()
    .any(|prefix| mnemonic.starts_with(prefix))
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
                ("rvv_state_errors", &RVV_STATE_ERRORS),
            ];
            for (name, counter) in counters {
                let _ = writeln!(file, "{name}={}", counter.load(Ordering::Relaxed));
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
    let output = args.iter().find_map(|arg| {
        let arg = unsafe { CStr::from_ptr(*arg) }.to_string_lossy();
        arg.strip_prefix("output=").map(PathBuf::from)
    });
    let Some(output) = output else {
        unsafe { qemu_plugin_outs(c"miniperf roofline: missing output=<path>\n".as_ptr()) };
        return -1;
    };
    if OUTPUT.set(output).is_err() {
        return -1;
    }

    unsafe {
        if target == Target::Riscv {
            qemu_plugin_register_vcpu_init_cb(id, initialize_vcpu);
        }
        qemu_plugin_register_vcpu_tb_trans_cb(id, translate_block);
        qemu_plugin_register_atexit_cb(id, plugin_exit, ptr::null_mut());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_riscv_scalar_and_vector_operations() {
        let scalar = classify(Target::Riscv, "fmadd.d fa0,fa1,fa2,fa3");
        assert_eq!(scalar.scalar_double, 2);

        assert_eq!(
            classify_rvv("vfmacc.vv v8,v9,v10,v0.t"),
            Some(RvvCost {
                kind: RvvKind::Float,
                factor: 2,
                masked: true,
                sew_scale: 1,
            })
        );
        assert_eq!(
            classify_rvv("vfwadd.vv v8,v9,v10"),
            Some(RvvCost {
                kind: RvvKind::Float,
                factor: 1,
                masked: false,
                sew_scale: 2,
            })
        );
    }

    #[test]
    fn decodes_rvv_sew_and_rejects_vill() {
        assert_eq!(rvv_sew(2 << 3, 64), Some(32));
        assert_eq!(rvv_sew(3 << 3, 64), Some(64));
        assert_eq!(rvv_sew(1_u64 << 63, 64), None);
    }

    #[test]
    fn classifies_x86_scalar_and_vector_operations() {
        let scalar = classify(Target::X86, "mulsd 0x8(%rax),%xmm0");
        assert_eq!(scalar.scalar_double, 1);

        let vector = classify(Target::X86, "vfmadd132ps %ymm1,%ymm2,%ymm3");
        assert_eq!(vector.vector_float, 16);

        let integer = classify(Target::X86, "vpaddd %zmm1,%zmm2,%zmm3");
        assert_eq!(integer.vector_int, 16);
    }

    #[test]
    fn does_not_treat_x86_simd_logic_as_scalar_integer_work() {
        let cost = classify(Target::X86, "orpd %xmm1,%xmm0");
        assert_eq!(cost.scalar_int, 0);
    }
}
