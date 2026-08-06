use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use mperf_data::{
    CallFrame, Event, EventType, Location, RooflineInfo, RooflineMethodInfo, ScenarioInfo,
};
use object::{Object, ObjectKind, ObjectSegment, ObjectSymbol, SegmentFlags};
use pmu::Process;
use serde::Serialize;
use smallvec::smallvec;

use super::{
    loops::ControlFlowGraph, BackendFuture, Options, ProfiledRun, RooflineBackend,
    monotonic_timestamp, profile_command,
};
use crate::event_dispatcher::EventDispatcher;

pub(super) struct QemuBackend {
    tool: AccountingTool,
    native_performance: bool,
    method: RooflineMethodInfo,
    cache: CacheInfo,
    memory_profile: bool,
}

/// Which same-artifact accounting tool drives run 2. Both emit the
/// `qemu-roofline.*` artifact set; DynamoRIO instruments natively and only
/// works when the guest matches the host architecture.
pub(super) enum AccountingTool {
    Qemu {
        qemu: Option<PathBuf>,
        plugin: PathBuf,
        extra_args: Vec<String>,
    },
    DynamoRio {
        drrun: PathBuf,
        client: PathBuf,
    },
}

/// Run-2 command line plus the extra environment it needs.
type AccountingCommand = (Vec<String>, Vec<(String, String)>);

impl AccountingTool {
    fn name(&self) -> &'static str {
        match self {
            AccountingTool::Qemu { .. } => "QEMU",
            AccountingTool::DynamoRio { .. } => "DynamoRIO",
        }
    }
}

#[derive(Default, Debug, Eq, PartialEq)]
struct Counts {
    instructions: u64,
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
    bytes_load: u64,
    bytes_store: u64,
    dram_bytes_load: u64,
    dram_bytes_store: u64,
    rvv_state_errors: u64,
    unclassified_instructions: u64,
    child_process_seen: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct BlockCounts {
    #[serde(skip_serializing)]
    end: u64,
    executions: u64,
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
    bytes_load: u64,
    bytes_store: u64,
    unclassified_instructions: u64,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct DynamicCfgCapture {
    image: Option<ImageInfo>,
    cache: Option<CacheInfo>,
    entries: BTreeSet<u64>,
    edges: BTreeMap<(u64, u64), u64>,
    blocks: BTreeMap<u64, BlockCounts>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheInfo {
    line_size: u64,
    capacity: u64,
    associativity: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImageInfo {
    start: u64,
    end: u64,
    entry: u64,
}

#[derive(Serialize)]
struct LoopArtifact {
    header: String,
    module_offset: String,
    function: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    latches: Vec<String>,
    blocks: Vec<String>,
    block_ranges: Vec<BlockRangeArtifact>,
    parent_header: Option<String>,
    trip_count: u64,
    inclusive: BlockCounts,
    self_counts: BlockCounts,
}

#[derive(Serialize)]
struct BlockRangeArtifact {
    runtime_start: String,
    runtime_end: String,
    module_start: String,
    module_end: String,
}

#[derive(Serialize)]
struct DynamicCfgArtifact {
    format_version: u32,
    model: &'static str,
    executable: String,
    warnings: Vec<&'static str>,
    cache_line_size: u64,
    llc_capacity: u64,
    llc_associativity: u64,
    traffic_model: &'static str,
    image_start: String,
    image_end: String,
    image_entry: String,
    load_bias: String,
    entries: Vec<String>,
    loops: Vec<LoopArtifact>,
    irreducible_cycles: Vec<Vec<String>>,
}

#[derive(Serialize)]
struct NativeTimingArtifact {
    pid: i32,
    start_ns: u64,
    end_ns: u64,
}

impl QemuBackend {
    #[cfg(not(target_os = "linux"))]
    pub(super) fn new(
        _options: &Options,
        _native_performance: bool,
        _method: RooflineMethodInfo,
        _memory_profile: bool,
    ) -> Result<Self> {
        anyhow::bail!("the QEMU roofline backend supports Linux hosts only");
    }

    #[cfg(target_os = "linux")]
    pub(super) fn new(
        options: &Options,
        native_performance: bool,
        method: RooflineMethodInfo,
        memory_profile: bool,
    ) -> Result<Self> {
        let plugin = plugin_path(options);
        if !plugin.is_file() {
            anyhow::bail!(
                "QEMU roofline plugin '{}' was not found; build it with `cargo build -p miniperf-qemu-roofline` or pass --qemu-plugin",
                plugin.display()
            );
        }

        Ok(Self {
            tool: AccountingTool::Qemu {
                qemu: options.qemu.clone(),
                plugin,
                extra_args: options.qemu_args.clone(),
            },
            native_performance,
            method,
            cache: detect_host_cache(),
            memory_profile,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn new_dynamorio(
        _options: &Options,
        _method: RooflineMethodInfo,
        _memory_profile: bool,
    ) -> Result<Self> {
        anyhow::bail!("the DynamoRIO roofline backend supports Linux hosts only");
    }

    #[cfg(target_os = "linux")]
    pub(super) fn new_dynamorio(
        options: &Options,
        method: RooflineMethodInfo,
        memory_profile: bool,
    ) -> Result<Self> {
        let (drrun, client) = dynamorio_paths(options)?;
        Ok(Self {
            tool: AccountingTool::DynamoRio { drrun, client },
            // DynamoRIO instruments the native binary; performance always
            // comes from a separate native run.
            native_performance: true,
            method,
            cache: detect_host_cache(),
            memory_profile,
        })
    }

    fn qemu_for(&self, guest: &Path) -> Result<PathBuf> {
        let AccountingTool::Qemu { qemu, .. } = &self.tool else {
            anyhow::bail!("qemu_for is only valid for the QEMU accounting tool");
        };
        if let Some(qemu) = qemu {
            return resolve_executable(qemu);
        }
        if let Some(qemu) = std::env::var_os("MPERF_QEMU") {
            return resolve_executable(&PathBuf::from(qemu));
        }

        let guest = resolve_executable(guest)?;
        let data = std::fs::read(&guest)
            .with_context(|| format!("read guest executable '{}'", guest.display()))?;
        let object = object::File::parse(data.as_slice())
            .with_context(|| format!("parse guest executable '{}'", guest.display()))?;
        if !matches!(object.kind(), ObjectKind::Executable | ObjectKind::Dynamic) {
            anyhow::bail!("QEMU roofline guest must be an executable ELF");
        }
        let binary = qemu_binary_for_architecture(object.architecture())
            .context("unsupported guest architecture for QEMU roofline")?;
        which::which(binary)
            .with_context(|| format!("could not find '{binary}'; pass --qemu explicitly"))
    }

    fn command(&self, qemu: &Path, guest: &[String], output: Option<&Path>) -> Result<Vec<String>> {
        let AccountingTool::Qemu {
            plugin, extra_args, ..
        } = &self.tool
        else {
            anyhow::bail!("command is only valid for the QEMU accounting tool");
        };
        let mut command = vec![qemu.to_string_lossy().to_string()];
        command.extend(extra_args.iter().cloned());
        if let Some(output) = output {
            let output = output.to_string_lossy();
            if output.contains(',') {
                anyhow::bail!("QEMU plugin output path cannot contain a comma");
            }
            command.push("-plugin".to_string());
            command.push(format!(
                "{},output={output},cache-line={},llc-size={},llc-assoc={},memory-profile={}",
                plugin.to_string_lossy(),
                self.cache.line_size,
                self.cache.capacity,
                self.cache.associativity,
                if self.memory_profile { "on" } else { "off" }
            ));
            if self.memory_profile {
                if let Some(preload) = option_env!("MPERF_MEMORY_PRELOAD") {
                    command.push("-E".to_string());
                    command.push(format!("LD_PRELOAD={preload}"));
                    command.push("-E".to_string());
                    command.push("MPERF_MEMORY_ALLOCATIONS=/dev/null".to_string());
                }
            }
        }
        command.extend(guest.iter().cloned());
        Ok(command)
    }

    /// Builds the accounting (run 2) command and its extra environment.
    fn accounting_command(&self, guest: &[String], output: &Path) -> Result<AccountingCommand> {
        match &self.tool {
            AccountingTool::Qemu { .. } => {
                let qemu = self.qemu_for(Path::new(&guest[0]))?;
                Ok((self.command(&qemu, guest, Some(output))?, Vec::new()))
            }
            AccountingTool::DynamoRio { drrun, client } => {
                let mut command = vec![
                    drrun.to_string_lossy().to_string(),
                    // Clean-call-heavy instrumentation exceeds DynamoRIO's
                    // block emit limits at the default 256-instruction blocks
                    // (observed on riscv64); traces add no value here.
                    "-disable_traces".to_string(),
                    "-max_bb_instrs".to_string(),
                    "32".to_string(),
                    "-c".to_string(),
                    client.to_string_lossy().to_string(),
                    format!("output={}", output.to_string_lossy()),
                    format!("cache-line={}", self.cache.line_size),
                    format!("llc-size={}", self.cache.capacity),
                    format!("llc-assoc={}", self.cache.associativity),
                    format!(
                        "memory-profile={}",
                        if self.memory_profile { "on" } else { "off" }
                    ),
                    "--".to_string(),
                ];
                command.extend(guest.iter().cloned());
                let mut env = Vec::new();
                if self.memory_profile {
                    if let Some(preload) = option_env!("MPERF_MEMORY_PRELOAD") {
                        env.push(("LD_PRELOAD".to_string(), preload.to_string()));
                        env.push((
                            "MPERF_MEMORY_ALLOCATIONS".to_string(),
                            "/dev/null".to_string(),
                        ));
                    }
                }
                Ok((command, env))
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn probe(_options: &Options, _guest: &Path) -> Result<()> {
    anyhow::bail!("the QEMU roofline backend supports Linux hosts only")
}

#[cfg(target_os = "linux")]
pub(super) fn probe(options: &Options, guest: &Path) -> Result<()> {
    let plugin = plugin_path(options);
    if !plugin.is_file() {
        anyhow::bail!("QEMU roofline plugin '{}' was not found", plugin.display());
    }
    let backend = QemuBackend {
        tool: AccountingTool::Qemu {
            qemu: options.qemu.clone(),
            plugin,
            extra_args: options.qemu_args.clone(),
        },
        native_performance: false,
        method: RooflineMethodInfo {
            selection: String::new(),
            accounting: String::new(),
            performance: String::new(),
            traffic: String::new(),
            quality: String::new(),
            reason: String::new(),
            warnings: Vec::new(),
        },
        cache: detect_host_cache(),
        memory_profile: false,
    };
    let qemu = backend.qemu_for(guest)?;
    ensure_plugin_support(&qemu)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn probe_dynamorio(_options: &Options) -> Result<()> {
    anyhow::bail!("the DynamoRIO roofline backend supports Linux hosts only")
}

/// Checks that drrun and the miniperf DynamoRIO client are available.
#[cfg(target_os = "linux")]
pub(super) fn probe_dynamorio(options: &Options) -> Result<()> {
    dynamorio_paths(options).map(|_| ())
}

#[cfg(target_os = "linux")]
fn dynamorio_paths(options: &Options) -> Result<(PathBuf, PathBuf)> {
    let drrun = options
        .dynamorio
        .clone()
        .or_else(|| std::env::var_os("MPERF_DYNAMORIO").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(|| {
            let bundled = super::executable_directory()
                .unwrap_or_default()
                .join("dynamorio/bin64/drrun");
            if bundled.is_file() {
                Ok(bundled)
            } else {
                which::which("drrun").map_err(|_| {
                    anyhow::anyhow!(
                        "DynamoRIO 'drrun' was not found next to mperf or on PATH; pass --dynamorio"
                    )
                })
            }
        })?;
    if !drrun.is_file() {
        anyhow::bail!("DynamoRIO launcher '{}' was not found", drrun.display());
    }
    let client = options
        .dynamorio_client
        .clone()
        .or_else(|| std::env::var_os("MPERF_DR_CLIENT").map(PathBuf::from))
        .unwrap_or_else(|| {
            super::executable_directory()
                .unwrap_or_default()
                .join("libdr_roofline.so")
        });
    if !client.is_file() {
        anyhow::bail!(
            "miniperf DynamoRIO client '{}' was not found; build utils/dr-roofline or pass --dynamorio-client",
            client.display()
        );
    }
    Ok((drrun, client))
}

#[cfg(target_os = "linux")]
fn detect_host_cache() -> CacheInfo {
    let fallback = CacheInfo {
        line_size: 64,
        capacity: 8 * 1024 * 1024,
        associativity: 16,
    };
    let Ok(indices) = std::fs::read_dir("/sys/devices/system/cpu/cpu0/cache") else {
        return fallback;
    };
    let mut candidates = Vec::new();
    for index in indices.flatten() {
        let path = index.path();
        let read = |name: &str| {
            std::fs::read_to_string(path.join(name))
                .ok()
                .map(|value| value.trim().to_owned())
        };
        let Some(cache_type) = read("type") else {
            continue;
        };
        if cache_type != "Unified" && cache_type != "Data" {
            continue;
        }
        let Some(level) = read("level").and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(capacity) = read("size").and_then(|value| parse_cache_size(&value)) else {
            continue;
        };
        let Some(line_size) = read("coherency_line_size").and_then(|value| value.parse().ok())
        else {
            continue;
        };
        let Some(associativity) =
            read("ways_of_associativity").and_then(|value| value.parse().ok())
        else {
            continue;
        };
        if line_size == 0 || associativity == 0 || capacity < line_size * associativity {
            continue;
        }
        candidates.push((
            level,
            CacheInfo {
                line_size,
                capacity,
                associativity,
            },
        ));
    }
    candidates
        .into_iter()
        .max_by_key(|(level, cache)| (*level, cache.capacity))
        .map(|(_, cache)| cache)
        .unwrap_or(fallback)
}

#[cfg(target_os = "linux")]
fn parse_cache_size(value: &str) -> Option<u64> {
    let (number, multiplier) = match value.as_bytes().last().copied()? {
        b'K' | b'k' => (&value[..value.len() - 1], 1024_u64),
        b'M' | b'm' => (&value[..value.len() - 1], 1024_u64 * 1024),
        b'G' | b'g' => (&value[..value.len() - 1], 1024_u64 * 1024 * 1024),
        _ => (value, 1),
    };
    number.trim().parse::<u64>().ok()?.checked_mul(multiplier)
}

#[cfg(target_os = "linux")]
fn plugin_path(options: &Options) -> PathBuf {
    options
        .qemu_plugin
        .clone()
        .or_else(|| std::env::var_os("MPERF_QEMU_PLUGIN").map(PathBuf::from))
        .unwrap_or_else(default_plugin_path)
}

impl RooflineBackend for QemuBackend {
    fn record<'a>(
        &'a self,
        dispatcher: Arc<EventDispatcher>,
        guest: &'a [String],
        output_directory: &'a Path,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            if let AccountingTool::Qemu { .. } = &self.tool {
                let qemu = self.qemu_for(Path::new(&guest[0]))?;
                ensure_plugin_support(&qemu)?;
            }
            let baseline_command = if self.native_performance {
                guest.to_vec()
            } else {
                let qemu = self.qemu_for(Path::new(&guest[0]))?;
                self.command(&qemu, guest, None)?
            };
            println!(
                "Run 1: collecting {} performance data for '{}'",
                if self.native_performance {
                    "native"
                } else {
                    "QEMU-hosted"
                },
                guest.join(" ")
            );
            let baseline = profile_command(
                dispatcher.clone(),
                &baseline_command,
                Vec::new(),
                false,
                self.memory_profile
                    .then(|| output_directory.join("memory-rss.txt")),
            )
            .await?;
            let timing_path = output_directory.join("memory-native.json");
            let timing_file = File::create(&timing_path)
                .with_context(|| format!("create '{}'", timing_path.display()))?;
            serde_json::to_writer(
                timing_file,
                &NativeTimingArtifact {
                    pid: baseline.pid,
                    start_ns: baseline.start_ns,
                    end_ns: baseline.end_ns,
                },
            )?;
            publish_baseline_region(
                dispatcher.clone(),
                baseline.pid,
                baseline.start_ns,
                baseline.end_ns,
                &guest[0],
            )
            .await;

            let counts_path = output_directory.join("qemu-roofline.counts");
            let (accounting_command, accounting_env) =
                self.accounting_command(guest, &counts_path)?;
            println!(
                "Run 2: collecting {} instruction and memory accounting for '{}'",
                self.tool.name(),
                guest.join(" ")
            );
            let process = Process::new(&accounting_command, &accounting_env)?;
            process.cont();
            process.wait()?;
            super::ensure_process_success(
                &process,
                &format!("{} Roofline accounting run", self.tool.name()),
            )?;
            let counts =
                parse_counts(&std::fs::read_to_string(&counts_path).with_context(|| {
                    format!(
                        "{} roofline accounting did not produce '{}'",
                        self.tool.name(),
                        counts_path.display()
                    )
                })?)?;
            let cfg_path = counts_path.with_extension("cfg");
            let cfg_capture =
                parse_dynamic_cfg(&std::fs::read_to_string(&cfg_path).with_context(|| {
                    format!(
                        "{} roofline accounting did not produce dynamic CFG '{}'",
                        self.tool.name(),
                        cfg_path.display()
                    )
                })?)?;
            validate_cfg_totals(&cfg_capture, &counts)?;
            let loops_path = output_directory.join("qemu-roofline.loops.json");
            let (natural_loops, irreducible_cycles) =
                write_loop_artifact(&loops_path, &cfg_capture, Path::new(&guest[0]))?;
            println!(
                "{} dynamic CFG: {natural_loops} natural loops, {irreducible_cycles} irreducible cycles; candidates saved to '{}'",
                self.tool.name(),
                loops_path.display()
            );
            if counts.rvv_state_errors != 0 {
                anyhow::bail!(
                    "{} roofline could not read RVV state for {} executed vector instructions",
                    self.tool.name(),
                    counts.rvv_state_errors
                );
            }
            if counts.unclassified_instructions != 0 {
                eprintln!(
                    "Warning: TMDL could not classify {} executed RISC-V instructions; Roofline operation totals are conservative",
                    counts.unclassified_instructions
                );
            }
            if counts.child_process_seen != 0 {
                eprintln!(
                    "Warning: child processes were detected and excluded from memory accounting"
                );
            }
            let mut method = self.method.clone();
            if baseline.instructions != 0 && counts.instructions != 0 {
                let difference = baseline.instructions.abs_diff(counts.instructions) as f64
                    / counts.instructions as f64;
                if difference > 0.05 {
                    let warning = format!(
                        "native and {} instruction totals differ by {:.1}% (native {}, instrumented {}); replay-dependent memory rates are degraded",
                        self.tool.name(),
                        difference * 100.0,
                        baseline.instructions,
                        counts.instructions
                    );
                    eprintln!("Warning: {warning}");
                    method.quality = "replay-diverged".to_string();
                    method.warnings.push(warning);
                }
            }
            if counts.child_process_seen != 0 {
                method.warnings.push(
                    "child processes were detected and excluded; v1 reports only the launched process and its threads"
                        .to_string(),
                );
            }
            publish_accounting_region(
                dispatcher,
                process.pid(),
                monotonic_timestamp()?,
                &guest[0],
                &counts,
            )
            .await;

            Ok(qemu_info(
                baseline,
                process.pid(),
                self.native_performance,
                method,
            ))
        })
    }
}

fn resolve_executable(executable: &Path) -> Result<PathBuf> {
    if executable.components().count() == 1 {
        which::which(executable)
            .with_context(|| format!("could not find QEMU executable '{}'", executable.display()))
    } else {
        Ok(executable.to_owned())
    }
}

fn ensure_plugin_support(qemu: &Path) -> Result<()> {
    let output = std::process::Command::new(qemu)
        .arg("--help")
        .output()
        .with_context(|| format!("run '{} --help'", qemu.display()))?;
    if !qemu_help_has_plugin(&output.stdout) && !qemu_help_has_plugin(&output.stderr) {
        anyhow::bail!(
            "QEMU executable '{}' has no TCG plugin support; use a QEMU user-mode build whose --help lists -plugin",
            qemu.display()
        );
    }
    let qemu_data = std::fs::read(qemu)
        .with_context(|| format!("read QEMU executable '{}'", qemu.display()))?;
    let qemu_object = object::File::parse(qemu_data.as_slice())
        .with_context(|| format!("parse QEMU executable '{}'", qemu.display()))?;
    let exported = qemu_object
        .dynamic_symbols()
        .filter_map(|symbol| symbol.name().ok())
        .collect::<BTreeSet<_>>();
    let missing = [
        "qemu_plugin_tb_vaddr",
        "qemu_plugin_insn_vaddr",
        "qemu_plugin_insn_size",
        "qemu_plugin_start_code",
        "qemu_plugin_end_code",
        "qemu_plugin_entry_code",
        "qemu_plugin_get_registers",
        "qemu_plugin_read_register",
    ]
    .into_iter()
    .filter(|symbol| !exported.contains(symbol))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!(
            "QEMU executable '{}' is missing plugin APIs required for accurate Roofline capture: {}",
            qemu.display(),
            missing.join(", ")
        );
    }
    Ok(())
}

fn qemu_help_has_plugin(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| line.trim_start().starts_with("-plugin"))
}

fn qemu_info(
    baseline: ProfiledRun,
    accounting_pid: i32,
    native_performance: bool,
    method: RooflineMethodInfo,
) -> ScenarioInfo {
    ScenarioInfo::Roofline(RooflineInfo {
        backend: if native_performance {
            "hybrid-qemu"
        } else {
            "qemu"
        }
        .to_string(),
        perf_pid: baseline.pid,
        counters: baseline.counters,
        inst_pid: accounting_pid,
        method: Some(Box::new(method)),
    })
}

async fn publish_baseline_region(
    dispatcher: Arc<EventDispatcher>,
    pid: i32,
    start: u64,
    end: u64,
    guest: &str,
) {
    let id = publish_start(&dispatcher, pid, start, guest).await;
    publish_end(&dispatcher, pid, end, id).await;
}

async fn publish_accounting_region(
    dispatcher: Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    guest: &str,
    counts: &Counts,
) {
    let id = publish_start(&dispatcher, pid, timestamp, guest).await;
    for (ty, value) in [
        (EventType::RooflineBytesLoad, counts.dram_bytes_load),
        (EventType::RooflineBytesStore, counts.dram_bytes_store),
        (EventType::RooflineScalarIntOps, counts.scalar_int_ops),
        (EventType::RooflineScalarFloatOps, counts.scalar_float_ops),
        (EventType::RooflineScalarDoubleOps, counts.scalar_double_ops),
        (EventType::RooflineVectorIntOps, counts.vector_int_ops),
        (EventType::RooflineVectorFloatOps, counts.vector_float_ops),
        (EventType::RooflineVectorDoubleOps, counts.vector_double_ops),
    ] {
        dispatcher
            .publish_event(synthetic_event(
                &dispatcher,
                pid,
                timestamp,
                ty,
                0,
                id,
                value,
            ))
            .await;
    }
    publish_end(&dispatcher, pid, timestamp, id).await;
}

async fn publish_start(
    dispatcher: &Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    guest: &str,
) -> u128 {
    let id = dispatcher.unique_id();
    let function_name = dispatcher.string_id_async("[QEMU whole process]").await;
    let file_name = dispatcher.string_id_async(guest).await;
    let mut event = synthetic_event(
        dispatcher,
        pid,
        timestamp,
        EventType::RooflineLoopStart,
        id,
        0,
        0,
    );
    event.callstack = smallvec![CallFrame::Location(Location {
        function_name,
        file_name,
        line: 0,
    })];
    dispatcher.publish_event(event).await;
    id
}

async fn publish_end(
    dispatcher: &Arc<EventDispatcher>,
    pid: i32,
    timestamp: u64,
    correlation_id: u128,
) {
    dispatcher
        .publish_event(synthetic_event(
            dispatcher,
            pid,
            timestamp,
            EventType::RooflineLoopEnd,
            0,
            correlation_id,
            0,
        ))
        .await;
}

fn synthetic_event(
    dispatcher: &EventDispatcher,
    pid: i32,
    timestamp: u64,
    ty: EventType,
    unique_id: u128,
    parent_or_correlation_id: u128,
    value: u64,
) -> Event {
    let is_counter = matches!(
        ty,
        EventType::RooflineBytesLoad
            | EventType::RooflineBytesStore
            | EventType::RooflineScalarIntOps
            | EventType::RooflineScalarFloatOps
            | EventType::RooflineScalarDoubleOps
            | EventType::RooflineVectorIntOps
            | EventType::RooflineVectorFloatOps
            | EventType::RooflineVectorDoubleOps
    );
    Event {
        unique_id: if unique_id == 0 {
            dispatcher.unique_id()
        } else {
            unique_id
        },
        correlation_id: if is_counter {
            0
        } else {
            parent_or_correlation_id
        },
        parent_id: if is_counter {
            parent_or_correlation_id
        } else {
            0
        },
        ty,
        thread_id: 0,
        process_id: pid as u32,
        cpu: u32::MAX,
        time_enabled: 0,
        time_running: 0,
        value,
        timestamp,
        name: 0,
        callstack: smallvec![],
        user_regs: None,
        user_stack: Vec::new(),
    }
}

fn parse_counts(input: &str) -> Result<Counts> {
    let mut values = HashMap::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line
            .split_once('=')
            .with_context(|| format!("invalid QEMU roofline count '{line}'"))?;
        let value = value
            .parse::<u64>()
            .with_context(|| format!("invalid QEMU roofline count '{line}'"))?;
        if values.insert(name, value).is_some() {
            anyhow::bail!("duplicate QEMU roofline count '{name}'");
        }
    }
    let get = |name| {
        values
            .get(name)
            .copied()
            .with_context(|| format!("QEMU roofline output is missing required count '{name}'"))
    };
    Ok(Counts {
        scalar_int_ops: get("scalar_int_ops")?,
        scalar_float_ops: get("scalar_float_ops")?,
        scalar_double_ops: get("scalar_double_ops")?,
        vector_int_ops: get("vector_int_ops")?,
        vector_float_ops: get("vector_float_ops")?,
        vector_double_ops: get("vector_double_ops")?,
        bytes_load: get("bytes_load")?,
        bytes_store: get("bytes_store")?,
        dram_bytes_load: get("dram_bytes_load")?,
        dram_bytes_store: get("dram_bytes_store")?,
        rvv_state_errors: get("rvv_state_errors")?,
        unclassified_instructions: get("unclassified_instructions")?,
        instructions: get("instructions")?,
        child_process_seen: get("child_process_seen")?,
    })
}

fn parse_dynamic_cfg(input: &str) -> Result<DynamicCfgCapture> {
    let mut capture = DynamicCfgCapture::default();
    let mut version_seen = false;
    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(version) = line.strip_prefix("miniperf-qemu-cfg=") {
            if version_seen {
                anyhow::bail!("duplicate QEMU dynamic CFG version");
            }
            if version != "3" {
                anyhow::bail!("unsupported QEMU dynamic CFG version '{version}'");
            }
            version_seen = true;
            continue;
        }

        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["cache", line_size, capacity, associativity, model]
                if matches!(*model, "write-back-no-rfo" | "write-back-write-allocate") =>
            {
                if capture.cache.is_some() {
                    anyhow::bail!("duplicate QEMU dynamic CFG cache metadata");
                }
                let cache = CacheInfo {
                    line_size: parse_decimal(line_size, line)?,
                    capacity: parse_decimal(capacity, line)?,
                    associativity: parse_decimal(associativity, line)?,
                };
                if !cache.line_size.is_power_of_two()
                    || cache.associativity == 0
                    || cache.capacity < cache.line_size * cache.associativity
                {
                    anyhow::bail!("invalid QEMU dynamic CFG cache metadata '{line}'");
                }
                capture.cache = Some(cache);
            }
            ["image", start, end, entry] => {
                if capture.image.is_some() {
                    anyhow::bail!("duplicate QEMU dynamic CFG image metadata");
                }
                let image = ImageInfo {
                    start: parse_address(start)?,
                    end: parse_address(end)?,
                    entry: parse_address(entry)?,
                };
                // For dynamically linked Linux executables QEMU reports the
                // interpreter's process entry, which legitimately lies outside
                // the main executable's text range. The main-image load bias is
                // derived from its executable ELF segment instead.
                if image.start >= image.end {
                    anyhow::bail!("invalid QEMU dynamic CFG image range '{line}'");
                }
                capture.image = Some(image);
            }
            ["entry", address] => {
                let address = parse_address(address)?;
                if !capture.entries.insert(address) {
                    anyhow::bail!("duplicate QEMU dynamic CFG entry '{address:#x}'");
                }
            }
            ["edge", from, to, executions] => {
                let edge = (parse_address(from)?, parse_address(to)?);
                let executions = parse_decimal(executions, line)?;
                if executions == 0 {
                    anyhow::bail!("QEMU dynamic CFG edge has zero executions: '{line}'");
                }
                if capture.edges.insert(edge, executions).is_some() {
                    anyhow::bail!(
                        "duplicate QEMU dynamic CFG edge '{:#x}' -> '{:#x}'",
                        edge.0,
                        edge.1
                    );
                }
            }
            ["block", address, end, executions, scalar_int, scalar_float, scalar_double, vector_int, vector_float, vector_double, bytes_load, bytes_store, unclassified] =>
            {
                let address = parse_address(address)?;
                let counts = BlockCounts {
                    end: parse_address(end)?,
                    executions: parse_decimal(executions, line)?,
                    scalar_int_ops: parse_decimal(scalar_int, line)?,
                    scalar_float_ops: parse_decimal(scalar_float, line)?,
                    scalar_double_ops: parse_decimal(scalar_double, line)?,
                    vector_int_ops: parse_decimal(vector_int, line)?,
                    vector_float_ops: parse_decimal(vector_float, line)?,
                    vector_double_ops: parse_decimal(vector_double, line)?,
                    bytes_load: parse_decimal(bytes_load, line)?,
                    bytes_store: parse_decimal(bytes_store, line)?,
                    unclassified_instructions: parse_decimal(unclassified, line)?,
                };
                if counts.executions == 0 {
                    anyhow::bail!("QEMU dynamic CFG block has zero executions: '{line}'");
                }
                if counts.end <= address {
                    anyhow::bail!("QEMU dynamic CFG block has an invalid range: '{line}'");
                }
                if capture.blocks.insert(address, counts).is_some() {
                    anyhow::bail!("duplicate QEMU dynamic CFG block '{address:#x}'");
                }
            }
            _ => anyhow::bail!("invalid QEMU dynamic CFG line '{line}'"),
        }
    }
    if !version_seen {
        anyhow::bail!("QEMU dynamic CFG has no format version");
    }
    if capture.blocks.is_empty() {
        anyhow::bail!("QEMU dynamic CFG contains no executed blocks");
    }
    if capture.image.is_none() {
        anyhow::bail!("QEMU dynamic CFG contains no main-image metadata");
    }
    if capture.cache.is_none() {
        anyhow::bail!("QEMU dynamic CFG contains no cache-model metadata");
    }
    for address in capture
        .entries
        .iter()
        .copied()
        .chain(capture.edges.keys().flat_map(|(from, to)| [*from, *to]))
    {
        if !capture.blocks.contains_key(&address) {
            anyhow::bail!("QEMU dynamic CFG references unknown block '{address:#x}'");
        }
    }
    Ok(capture)
}

fn parse_address(value: &str) -> Result<u64> {
    let value = value
        .strip_prefix("0x")
        .with_context(|| format!("QEMU dynamic CFG address is not hexadecimal: '{value}'"))?;
    u64::from_str_radix(value, 16)
        .with_context(|| format!("invalid QEMU dynamic CFG address '0x{value}'"))
}

fn parse_decimal(value: &str, line: &str) -> Result<u64> {
    value
        .parse()
        .with_context(|| format!("invalid QEMU dynamic CFG count in '{line}'"))
}

fn validate_cfg_totals(capture: &DynamicCfgCapture, counts: &Counts) -> Result<()> {
    let totals = sum_blocks(capture.blocks.values());
    let expected = BlockCounts {
        scalar_int_ops: counts.scalar_int_ops,
        scalar_float_ops: counts.scalar_float_ops,
        scalar_double_ops: counts.scalar_double_ops,
        vector_int_ops: counts.vector_int_ops,
        vector_float_ops: counts.vector_float_ops,
        vector_double_ops: counts.vector_double_ops,
        bytes_load: counts.dram_bytes_load,
        bytes_store: counts.dram_bytes_store,
        unclassified_instructions: counts.unclassified_instructions,
        ..BlockCounts::default()
    };
    let mut comparable = totals;
    comparable.executions = 0;
    // Dirty lines still resident at process exit are flushed into the
    // whole-process traffic total. They have no honest final basic-block
    // owner, so per-block stores may be smaller but can never be larger.
    let stores_are_valid = comparable.bytes_store <= expected.bytes_store;
    comparable.bytes_store = expected.bytes_store;
    if !stores_are_valid || comparable != expected {
        anyhow::bail!(
            "QEMU per-block accounting does not match whole-process totals: blocks={comparable:?}, process={expected:?}"
        );
    }
    Ok(())
}

fn write_loop_artifact(
    path: &Path,
    capture: &DynamicCfgCapture,
    executable: &Path,
) -> Result<(usize, usize)> {
    let image = capture
        .image
        .context("QEMU dynamic CFG has no image metadata")?;
    let object_data = std::fs::read(executable)
        .with_context(|| format!("read Roofline executable '{}'", executable.display()))?;
    let object = object::File::parse(object_data.as_slice())
        .with_context(|| format!("parse Roofline executable '{}'", executable.display()))?;
    let link_text_start = executable_segment_start(&object)
        .context("Roofline executable has no executable ELF load segment")?;
    let load_bias = image.start.checked_sub(link_text_start).with_context(|| {
        format!(
            "QEMU main-image start {:#x} precedes executable segment {link_text_start:#x}",
            image.start
        )
    })?;
    let runtime_entry = object
        .entry()
        .checked_add(load_bias)
        .context("Roofline executable entry address overflow")?;
    if !((image.start..image.end).contains(&runtime_entry)) {
        anyhow::bail!(
            "derived main executable entry {runtime_entry:#x} is outside QEMU image {:#x}..{:#x}",
            image.start,
            image.end
        );
    }
    let loader = addr2line::Loader::new(executable).ok();
    let in_main_image = |address: u64| address >= image.start && address < image.end;
    let main_blocks = capture
        .blocks
        .keys()
        .copied()
        .filter(|address| in_main_image(*address))
        .collect::<BTreeSet<_>>();
    if main_blocks.is_empty() {
        anyhow::bail!("QEMU dynamic CFG has no blocks in the main executable image");
    }
    let mut graph = ControlFlowGraph::default();
    let mut main_entries = capture
        .entries
        .iter()
        .copied()
        .filter(|address| in_main_image(*address))
        .collect::<BTreeSet<_>>();
    for &block in &main_blocks {
        let has_main_predecessor = capture
            .edges
            .keys()
            .any(|(from, to)| *to == block && main_blocks.contains(from));
        if !has_main_predecessor {
            main_entries.insert(block);
        }
    }
    let artifact_entries = main_entries.iter().copied().map(hex).collect::<Vec<_>>();
    for entry in main_entries {
        graph.add_entry(entry);
    }
    for &(from, to) in capture.edges.keys() {
        if main_blocks.contains(&from) && main_blocks.contains(&to) {
            graph.add_edge(from, to);
        }
    }
    let analysis = graph.analyze();
    let loops = analysis
        .natural_loops
        .iter()
        .map(|loop_info| {
            let module_offset = loop_info.header.saturating_sub(load_bias);
            let (function, file, line) = resolve_source(loader.as_ref(), module_offset);
            let self_blocks = loop_info
                .blocks
                .iter()
                .filter(|block| {
                    !analysis.natural_loops.iter().any(|candidate| {
                        candidate.header != loop_info.header
                            && candidate.blocks.len() < loop_info.blocks.len()
                            && loop_info.blocks.is_superset(&candidate.blocks)
                            && candidate.blocks.contains(block)
                    })
                })
                .filter_map(|address| capture.blocks.get(address));
            LoopArtifact {
                header: hex(loop_info.header),
                module_offset: hex(module_offset),
                function,
                file,
                line,
                latches: loop_info.latches.iter().copied().map(hex).collect(),
                blocks: loop_info.blocks.iter().copied().map(hex).collect(),
                block_ranges: loop_info
                    .blocks
                    .iter()
                    .filter_map(|start| {
                        capture.blocks.get(start).map(|counts| BlockRangeArtifact {
                            runtime_start: hex(*start),
                            runtime_end: hex(counts.end),
                            module_start: hex(start.saturating_sub(load_bias)),
                            module_end: hex(counts.end.saturating_sub(load_bias)),
                        })
                    })
                    .collect(),
                parent_header: loop_info
                    .parent
                    .map(|parent| hex(analysis.natural_loops[parent].header)),
                trip_count: loop_info
                    .latches
                    .iter()
                    .filter_map(|latch| capture.edges.get(&(*latch, loop_info.header)))
                    .copied()
                    .sum(),
                inclusive: sum_blocks(
                    loop_info
                        .blocks
                        .iter()
                        .filter_map(|address| capture.blocks.get(address)),
                ),
                self_counts: sum_blocks(self_blocks),
            }
        })
        .collect::<Vec<_>>();
    let irreducible_cycles = analysis
        .irreducible_cycles
        .iter()
        .map(|cycle| cycle.iter().copied().map(hex).collect())
        .collect::<Vec<_>>();
    let cache = capture
        .cache
        .context("QEMU dynamic CFG has no cache-model metadata")?;
    let artifact = DynamicCfgArtifact {
        format_version: 3,
        model: "observed QEMU translation-block CFG with summarized calls",
        executable: executable.to_string_lossy().into_owned(),
        warnings: vec![
            "Only control-flow edges executed during the accounting run are present.",
            "Call/return pairs are summarized; non-local control transfers can split a region.",
            "Loop bytes are deterministic shared-LLC model traffic, not hardware memory-controller measurements.",
            "The cache model uses write allocation/RFO and dirty writeback; non-temporal stores are conservative.",
            "Source locations require debug information; optimized code may map multiple loops to one line.",
        ],
        cache_line_size: cache.line_size,
        llc_capacity: cache.capacity,
        llc_associativity: cache.associativity,
        traffic_model: "shared-llc-write-back-write-allocate",
        image_start: hex(image.start),
        image_end: hex(image.end),
        image_entry: hex(runtime_entry),
        load_bias: hex(load_bias),
        entries: artifact_entries,
        loops,
        irreducible_cycles,
    };
    let natural_count = artifact.loops.len();
    let irreducible_count = artifact.irreducible_cycles.len();
    serde_json::to_writer_pretty(
        File::create(path)
            .with_context(|| format!("create QEMU loop artifact '{}'", path.display()))?,
        &artifact,
    )
    .with_context(|| format!("write QEMU loop artifact '{}'", path.display()))?;
    Ok((natural_count, irreducible_count))
}

fn executable_segment_start(object: &object::File<'_>) -> Option<u64> {
    object
        .segments()
        .filter(|segment| {
            matches!(
                segment.flags(),
                SegmentFlags::Elf { p_flags } if p_flags & object::elf::PF_X != 0
            )
        })
        .map(|segment| segment.address())
        .min()
}

fn resolve_source(
    loader: Option<&addr2line::Loader>,
    address: u64,
) -> (Option<String>, Option<String>, Option<u32>) {
    let Some(loader) = loader else {
        return (None, None, None);
    };
    let mut function = loader
        .find_symbol(address)
        .map(|symbol| addr2line::demangle_auto(Cow::Borrowed(symbol), None).into_owned());
    let mut file = None;
    let mut line = None;
    if let Ok(mut frames) = loader.find_frames(address) {
        while let Ok(Some(frame)) = frames.next() {
            if function.is_none() {
                function = frame
                    .function
                    .and_then(|name| name.demangle().ok().map(Cow::into_owned));
            }
            if file.is_none() {
                file = frame
                    .location
                    .as_ref()
                    .and_then(|location| location.file.map(str::to_owned));
            }
            if line.is_none() {
                line = frame.location.and_then(|location| location.line);
            }
            if function.is_some() && file.is_some() && line.is_some() {
                break;
            }
        }
    }
    (function, file, line)
}

fn sum_blocks<'a>(blocks: impl IntoIterator<Item = &'a BlockCounts>) -> BlockCounts {
    let mut total = BlockCounts::default();
    for block in blocks {
        total.executions = total.executions.saturating_add(block.executions);
        total.scalar_int_ops = total.scalar_int_ops.saturating_add(block.scalar_int_ops);
        total.scalar_float_ops = total
            .scalar_float_ops
            .saturating_add(block.scalar_float_ops);
        total.scalar_double_ops = total
            .scalar_double_ops
            .saturating_add(block.scalar_double_ops);
        total.vector_int_ops = total.vector_int_ops.saturating_add(block.vector_int_ops);
        total.vector_float_ops = total
            .vector_float_ops
            .saturating_add(block.vector_float_ops);
        total.vector_double_ops = total
            .vector_double_ops
            .saturating_add(block.vector_double_ops);
        total.bytes_load = total.bytes_load.saturating_add(block.bytes_load);
        total.bytes_store = total.bytes_store.saturating_add(block.bytes_store);
        total.unclassified_instructions = total
            .unclassified_instructions
            .saturating_add(block.unclassified_instructions);
    }
    total
}

fn hex(address: u64) -> String {
    format!("{address:#x}")
}

#[cfg(target_os = "linux")]
fn default_plugin_path() -> PathBuf {
    super::executable_directory()
        .unwrap_or_default()
        .join("libminiperf_qemu_roofline.so")
}

fn qemu_binary_for_architecture(architecture: object::Architecture) -> Option<&'static str> {
    match architecture {
        object::Architecture::Riscv64 => Some("qemu-riscv64"),
        object::Architecture::Riscv32 => Some("qemu-riscv32"),
        object::Architecture::X86_64 => Some("qemu-x86_64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_counts() {
        let counts = parse_counts(
            "scalar_int_ops=7\nscalar_float_ops=0\nscalar_double_ops=3\nvector_int_ops=0\nvector_float_ops=2\nvector_double_ops=0\nbytes_load=64\nbytes_store=32\ndram_bytes_load=128\ndram_bytes_store=64\nrvv_state_errors=0\nunclassified_instructions=5\ninstructions=99\nchild_process_seen=0\n",
        )
        .unwrap();
        assert_eq!(
            counts,
            Counts {
                scalar_int_ops: 7,
                scalar_double_ops: 3,
                vector_float_ops: 2,
                bytes_load: 64,
                bytes_store: 32,
                dram_bytes_load: 128,
                dram_bytes_store: 64,
                unclassified_instructions: 5,
                instructions: 99,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn rejects_incomplete_or_duplicate_plugin_counts() {
        let missing = parse_counts("scalar_int_ops=1\n").unwrap_err().to_string();
        assert!(missing.contains("missing required count"));

        let duplicate = parse_counts(
            "scalar_int_ops=1\nscalar_int_ops=2\nscalar_float_ops=0\nscalar_double_ops=0\nvector_int_ops=0\nvector_float_ops=0\nvector_double_ops=0\nbytes_load=0\nbytes_store=0\ndram_bytes_load=0\ndram_bytes_store=0\nrvv_state_errors=0\nunclassified_instructions=0\ninstructions=0\nchild_process_seen=0\n",
        )
        .unwrap_err()
        .to_string();
        assert!(duplicate.contains("duplicate"));
    }

    #[test]
    fn parses_validates_and_analyzes_dynamic_cfg() {
        let executable = Path::new("/bin/true");
        let object_data = std::fs::read(executable).unwrap();
        let object = object::File::parse(object_data.as_slice()).unwrap();
        let base = executable_segment_start(&object).unwrap();
        let image_end = object.entry().max(base + 0x100) + 1;
        let external = image_end + 0x100;
        let capture = parse_dynamic_cfg(&format!(
            "miniperf-qemu-cfg=3\n\
             cache 64 8388608 16 write-back-no-rfo\n\
             image {base:#x} {image_end:#x} {external:#x}\n\
             entry {base:#x}\n\
             edge {base:#x} {:#x} 1\n\
             edge {:#x} {:#x} 3\n\
             edge {:#x} {:#x} 1\n\
             edge {:#x} {:#x} 2\n\
             entry {external:#x}\n\
             edge {external:#x} {:#x} 1\n\
             edge {:#x} {external:#x} 1\n\
             block {base:#x} {:#x} 1 1 0 0 0 0 0 0 0 0\n\
             block {:#x} {:#x} 3 3 0 0 0 0 0 24 0 0\n\
             block {:#x} {:#x} 3 3 0 0 0 0 0 0 12 0\n\
             block {:#x} {:#x} 1 0 0 0 0 0 0 0 0 0\n\
             block {external:#x} {:#x} 1 1 0 0 0 0 0 0 0 0\n\
             block {:#x} {:#x} 1 1 0 0 0 0 0 0 0 0\n",
            base + 0x10,
            base + 0x10,
            base + 0x20,
            base + 0x10,
            base + 0x30,
            base + 0x20,
            base + 0x10,
            external + 0x10,
            external + 0x10,
            base + 0x10,
            base + 0x10,
            base + 0x20,
            base + 0x20,
            base + 0x30,
            base + 0x30,
            base + 0x40,
            external + 0x10,
            external + 0x10,
            external + 0x20,
        ))
        .unwrap();
        let counts = Counts {
            scalar_int_ops: 9,
            bytes_load: 24,
            bytes_store: 12,
            dram_bytes_load: 24,
            dram_bytes_store: 12,
            ..Counts::default()
        };
        validate_cfg_totals(&capture, &counts).unwrap();

        let path = std::env::temp_dir().join(format!(
            "mperf-qemu-loop-test-{}.json",
            uuid::Uuid::now_v7()
        ));
        assert_eq!(
            write_loop_artifact(&path, &capture, executable).unwrap(),
            (1, 0)
        );
        let artifact: serde_json::Value =
            serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        assert_eq!(artifact["loops"][0]["header"], hex(base + 0x10));
        assert_eq!(artifact["loops"][0]["trip_count"], 2);
        assert_eq!(artifact["loops"][0]["inclusive"]["scalar_int_ops"], 6);
        assert_eq!(
            artifact["loops"][0]["block_ranges"][0]["runtime_start"],
            hex(base + 0x10)
        );
        assert_eq!(
            artifact["loops"][0]["block_ranges"][0]["runtime_end"],
            hex(base + 0x20)
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_cfg_with_unknown_blocks_or_bad_totals() {
        let unknown = parse_dynamic_cfg(
            "miniperf-qemu-cfg=3\ncache 64 8388608 16 write-back-no-rfo\nimage 0x100 0x300 0x100\nentry 0x100\nedge 0x100 0x200 1\nblock 0x100 0x110 1 0 0 0 0 0 0 0 0 0\n",
        )
        .unwrap_err()
        .to_string();
        assert!(unknown.contains("unknown block"));

        let capture = parse_dynamic_cfg(
            "miniperf-qemu-cfg=3\ncache 64 8388608 16 write-back-no-rfo\nimage 0x100 0x200 0x100\nentry 0x100\nblock 0x100 0x110 1 1 0 0 0 0 0 0 0 0\n",
        )
        .unwrap();
        let mismatch = validate_cfg_totals(&capture, &Counts::default())
            .unwrap_err()
            .to_string();
        assert!(mismatch.contains("does not match"));
    }

    #[test]
    fn maps_supported_guests_to_user_mode_qemu() {
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::Riscv64),
            Some("qemu-riscv64")
        );
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::Riscv32),
            Some("qemu-riscv32")
        );
        assert_eq!(
            qemu_binary_for_architecture(object::Architecture::X86_64),
            Some("qemu-x86_64")
        );
    }

    #[test]
    fn recognizes_qemu_plugin_help() {
        assert!(qemu_help_has_plugin(
            b"  -plugin [file=]file[,arg=<string>]\n"
        ));
        assert!(!qemu_help_has_plugin(b"  -cpu model\n"));
    }
}
