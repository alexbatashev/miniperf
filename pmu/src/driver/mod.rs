#[cfg(target_os = "linux")]
pub(crate) mod perf;

#[cfg(target_os = "macos")]
mod kperf;

#[cfg(target_os = "linux")]
use perf::{PerfCountingDriver, PerfSamplingDriver};

#[cfg(target_os = "macos")]
use kperf::{KPerfCountingDriver, KPerfSamplingDriver};

use itertools::chain;
use smallvec::SmallVec;
use std::sync::Arc;

use crate::{cpu_family, Counter, Error, Process};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Operating-system backend used to access performance counters.
pub enum DriverKind {
    /// Select the native backend automatically.
    Default,
    /// Linux `perf_event_open` backend.
    Perf,
    /// Apple kperf backend.
    KPerf,
}

/// Strategy used to collect user-space call stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnwindMode {
    /// Request the kernel's frame-pointer callchain.
    FramePointer,
    #[default]
    /// Capture registers and stack bytes for post-hoc DWARF unwinding.
    Dwarf,
}

/// Register state captured by `PERF_SAMPLE_REGS_USER`.
#[derive(Debug, Clone)]
pub struct UserRegs {
    /// Perf register ABI tag.
    pub abi: u64,
    /// Bit mask identifying captured architecture registers.
    pub mask: u64,
    /// Values are ordered by increasing set-bit index in `mask`.
    pub values: Vec<u64>,
}

#[derive(Debug, Clone)]
/// One counter value and its perf multiplexing scale.
pub struct CounterValue {
    /// Scaled counter value.
    pub value: u64,
    /// Ratio of enabled time to running time.
    pub scaling: f64,
    /// Reliability of the value after multiplexing or estimation.
    pub quality: MeasurementQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Reliability classification for a counter measurement.
pub enum MeasurementQuality {
    /// Directly observed without multiplexing.
    Exact,
    /// Multiplexed and scaled by enabled/running time.
    Scaled,
    /// Estimated from an indirect platform source.
    Estimated,
}

/// Counting driver is used for simple collection of system's performance counters values. On Linux,
/// counter multiplexing is supported.
pub trait CountingDriver {
    /// Enables configured counters.
    fn start(&mut self) -> Result<(), Error>;
    /// Disables configured counters.
    fn stop(&mut self) -> Result<(), Error>;
    /// Resets configured counter values to zero.
    fn reset(&mut self) -> Result<(), Error>;
    /// Reads the current counter values.
    fn counters(&mut self) -> Result<CounterResult, std::io::Error>;
}

/// Receives records from a sampling driver.
pub trait SamplingCallback: Send + Sync {
    /// Handles one sample or process mapping record.
    fn call(&self, record: Record);
}

/// Common interface for streaming PMU samples.
pub trait SamplingDriver {
    /// Counters that were successfully activated after capability fallbacks.
    fn counters(&self) -> Vec<Counter>;

    /// Starts sampling and forwards records to `callback`.
    fn start(&mut self, callback: Arc<dyn SamplingCallback>) -> Result<(), Error>;

    /// Stops sampling, drains pending records, and joins the reader thread.
    fn stop(&mut self) -> Result<(), Error>;
}

/// Identifies the core cluster a counter value was measured on, on a
/// heterogeneous (big.LITTLE) system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreId {
    /// Family id, e.g. `"cortex_a720"`.
    pub family_id: String,
    /// Human readable name, e.g. `"ARM Cortex-A720"`.
    pub name: String,
    /// sysfs cpumask for the cluster, e.g. `"0,5-11"`.
    pub cpus: String,
}

/// A single measured counter value, tagged with the core it was measured on.
#[derive(Debug, Clone)]
pub struct CounterEntry {
    /// The core cluster this value came from. `None` on homogeneous systems and
    /// for software counters, which are not PMU-specific.
    pub core: Option<CoreId>,
    /// Counter that produced this entry.
    pub counter: Counter,
    /// Measured value and scale.
    pub value: CounterValue,
}

#[derive(Debug, Clone, Default)]
/// Values returned by a counting driver.
pub struct CounterResult {
    entries: SmallVec<[CounterEntry; 16]>,
}

/// Sampling driver produces records that describe events
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Keep samples inline on the sampling hot path.
pub enum Record {
    /// A performance-counter sample.
    Sample(Sample),
    /// A precise memory-access sample (PEBS/IBS/SPE).
    MemSample(MemSample),
    /// A process address-space mapping.
    ProcAddr(ProcAddr),
}

/// One precise memory access: skid-free IP plus the data address, vendor data
/// source encoding and access latency the hardware reported for it.
#[derive(Debug)]
pub struct MemSample {
    /// Instruction pointer of the accessing instruction.
    pub ip: u64,
    /// Process ID.
    pub pid: u32,
    /// Thread ID.
    pub tid: u32,
    /// CPU the access executed on.
    pub cpu: u32,
    /// Timestamp on `CLOCK_MONOTONIC`.
    pub time: u64,
    /// Virtual address the access targeted.
    pub data_addr: u64,
    /// Access latency in core cycles, zero when unreported.
    pub latency: u64,
    /// Raw vendor data-source encoding, normalized downstream.
    pub data_src: u64,
    /// Kernel-provided instruction-pointer callchain.
    pub callstack: SmallVec<[u64; 8]>,
    /// Call stack reconstructed from the hardware branch records.
    pub lbr_callstack: SmallVec<[u64; 8]>,
    /// Raw user register state for post-hoc unwinding.
    pub user_regs: Option<UserRegs>,
    /// User stack bytes beginning at the sampled stack pointer.
    pub user_stack: Vec<u8>,
}

/// A structure that represents a single sample
#[derive(Debug)]
pub struct Sample {
    /// Unique ID shared by all samples of the event
    pub event_id: u128,
    /// Instruction pointer
    pub ip: u64,
    /// Process ID
    pub pid: u32,
    /// Thread ID
    pub tid: u32,
    /// CPU ID that the event occured on
    pub cpu: u32,
    /// Family id of the core cluster this sample came from (e.g.
    /// `"cortex_a720"`), on a heterogeneous system. `None` on homogeneous hosts.
    pub core: Option<String>,
    /// Timestamp
    pub time: u64,
    /// Time for which the event was enabled.
    pub time_enabled: u64,
    /// Time for which the event was scheduled on hardware.
    pub time_running: u64,
    /// Counter represented by this sample.
    pub counter: Counter,
    /// Counter delta since the preceding sample.
    pub value: u64,
    /// Kernel-provided instruction-pointer callchain.
    pub callstack: SmallVec<[u64; 8]>,
    /// Call stack reconstructed from the hardware branch stack (Intel LBR
    /// call-stack mode). Empty when branch records were not requested.
    pub lbr_callstack: SmallVec<[u64; 8]>,
    /// Raw user register state for post-hoc unwinding.
    pub user_regs: Option<UserRegs>,
    /// User stack bytes beginning at the sampled stack pointer.
    pub user_stack: Vec<u8>,
}

#[derive(Debug)]
/// One process memory mapping observed by perf.
pub struct ProcAddr {
    /// Process identifier.
    pub pid: u32,
    /// Mapping start address.
    pub addr: u64,
    /// Mapping length in bytes.
    pub len: u64,
    /// File offset backing the mapping.
    pub pgoff: u64,
    /// Path of the mapped file.
    pub filename: String,
}

/// Builder for a counting driver.
pub struct CountingDriverBuilder {
    counters: Vec<Counter>,
    pid: Option<i32>,
    kind: DriverKind,
}

/// Builder for a sampling driver.
pub struct SamplingDriverBuilder {
    counters: Vec<Counter>,
    sample_freq: u64,
    pid: Option<i32>,
    prefer_raw_events: bool,
    kind: DriverKind,
    unwind_mode: UnwindMode,
    stack_dump_size: u32,
    precise_ip: bool,
    lbr_callstack: bool,
}

impl<F: Fn(Record) + Send + Sync> SamplingCallback for F {
    fn call(&self, record: Record) {
        self(record)
    }
}

/// Open precise memory sampling (data address, data source and latency per
/// access) for a target process, alongside whatever counter-based sampling is
/// already running: Arm SPE where an `arm_spe_*` PMU exists, Intel PEBS
/// `mem-loads`/`mem-stores` otherwise.
#[cfg(target_os = "linux")]
pub fn mem_sampling_driver(
    pid: i32,
    sample_freq: u64,
    stack_dump_size: u32,
    lbr_callstack: bool,
) -> Result<Box<dyn SamplingDriver>, Error> {
    if perf::spe_pmu_path().is_some() {
        return Ok(Box::new(perf::PerfSpeSamplingDriver::new(pid)?));
    }
    #[cfg(target_arch = "x86_64")]
    return Ok(Box::new(perf::PerfMemSamplingDriver::new(
        pid,
        sample_freq,
        stack_dump_size,
        perf::dwarf_register_mask(),
        lbr_callstack,
    )?));
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (sample_freq, stack_dump_size, lbr_callstack);
        Err(Error::InvalidConfiguration(
            "no precise memory sampling facility on this host".to_owned(),
        ))
    }
}

/// The precise-memory facility `mem_sampling_driver` would use, for status
/// reporting: `"spe"` or `"pebs"`.
#[cfg(target_os = "linux")]
pub fn mem_sampling_kind() -> &'static str {
    if perf::spe_pmu_path().is_some() {
        "spe"
    } else {
        "pebs"
    }
}

/// Lists counters known to the selected host driver and event table.
pub fn list_supported_counters(driver: DriverKind) -> Vec<Counter> {
    cfg_if::cfg_if! {
        if #[cfg(target_os="linux")] {
            if driver == DriverKind::Default || driver == DriverKind::Perf {
                return perf::list_supported_counters();
            }
        } else if #[cfg(target_os="macos")] {
            if driver == DriverKind::Default || driver == DriverKind::KPerf {
                return kperf::list_supported_counters();
            }
        }
    }

    vec![]
}

impl CountingDriverBuilder {
    /// Creates an empty counting-driver configuration.
    pub fn new() -> Self {
        CountingDriverBuilder {
            counters: vec![],
            pid: None,
            kind: DriverKind::Default,
        }
    }

    /// Selects counters to collect.
    pub fn counters(mut self, counters: &[Counter]) -> Self {
        self.counters = counters.to_vec();
        self
    }

    /// Selects a child process, or the current thread when `None`.
    pub fn process(mut self, process: Option<&Process>) -> Self {
        self.pid = process.map(|p| p.pid());
        self
    }

    /// Attaches counting to an already-running process.
    pub fn pid(mut self, pid: Option<i32>) -> Self {
        // `process()` supplies a suspended child PID. Callers commonly chain
        // `.pid(optional_pid)` afterwards; an absent optional attachment must
        // not silently replace that child with PID 0 (the profiler itself).
        if pid.is_some() {
            self.pid = pid;
        }
        self
    }

    /// Opens the configured counters and returns the native driver.
    pub fn build(self) -> Result<Box<dyn CountingDriver>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os="linux")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::Perf {
                    return Ok(Box::new(PerfCountingDriver::new(self.counters, self.pid)?));
                }
            } else if #[cfg(target_os="macos")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::KPerf {
                    return Ok(Box::new(KPerfCountingDriver::new(self.counters, self.pid)?));
                }
            }
        }

        Err(Error::UnsupportedDriver {
            driver: format!("{:?}", self.kind),
        })
    }
}

impl Default for CountingDriverBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplingDriverBuilder {
    /// Creates a sampling configuration with a 1 kHz rate and DWARF stacks.
    pub fn new() -> Self {
        SamplingDriverBuilder {
            counters: vec![],
            sample_freq: crate::DEFAULT_SAMPLE_FREQUENCY_HZ,
            pid: None,
            prefer_raw_events: true,
            kind: DriverKind::Default,
            unwind_mode: UnwindMode::Dwarf,
            stack_dump_size: 8 * 1024,
            precise_ip: false,
            lbr_callstack: false,
        }
    }

    /// Selects counters included in each sample.
    pub fn counters(mut self, counters: &[Counter]) -> Self {
        let cpu_family = cpu_family::get_host_cpu_family();
        let info = cpu_family::find_cpu_family(cpu_family);

        let leader = info.and_then(|info| info.leader_event.clone());

        let counters = if let Some(leader) = leader {
            chain([Counter::Custom(leader)], counters.iter().cloned()).collect()
        } else {
            counters.to_vec()
        };

        self.counters = counters;
        self
    }

    /// Attaches sampling to a suspended child process.
    pub fn process(mut self, process: &Process) -> Self {
        self.pid = Some(process.pid());
        self
    }

    /// Attaches sampling to an already-running process.
    pub fn pid(mut self, pid: i32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Sets the target interrupt frequency in hertz.
    pub fn sample_freq(mut self, sample_freq: u64) -> Self {
        self.sample_freq = sample_freq;
        self
    }

    /// Selects the user call-stack collection strategy.
    pub fn unwind_mode(mut self, unwind_mode: UnwindMode) -> Self {
        self.unwind_mode = unwind_mode;
        self
    }

    /// Requests hardware branch records in call-stack mode (Intel LBR) as an
    /// extra stack source alongside the selected unwind mode. Silently ignored
    /// where the hardware or kernel rejects it.
    pub fn lbr_callstack(mut self, enabled: bool) -> Self {
        self.lbr_callstack = enabled;
        self
    }

    /// Maximum number of user stack bytes captured for each DWARF sample.
    pub fn stack_dump_size(mut self, bytes: u32) -> Self {
        self.stack_dump_size = bytes;
        self
    }

    /// Requests PEBS/SPE-quality instruction pointers for supported events.
    /// Opening the event remains the kernel's authoritative capability check.
    pub fn precise_ip(mut self) -> Self {
        self.precise_ip = true;
        self
    }

    /// Prefers raw CPU-family event encodings over generic perf aliases.
    pub fn prefer_raw_events(mut self) -> Self {
        self.prefer_raw_events = true;
        self
    }

    /// Opens events and creates the native sampling driver.
    pub fn build(self) -> Result<Box<dyn SamplingDriver>, Error> {
        cfg_if::cfg_if! {
            if #[cfg(target_os="linux")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::Perf {
                    let options = perf::SampleOptions {
                        sample_freq: self.sample_freq,
                        unwind_mode: self.unwind_mode,
                        stack_dump_size: self.stack_dump_size,
                        precise_ip: self.precise_ip,
                        branch_mode: None,
                    };
                    let driver = sampling_with_fallback(
                        self.counters,
                        self.lbr_callstack,
                        |counters, branch_mode| PerfSamplingDriver::new(
                            counters,
                            &perf::SampleOptions {
                                branch_mode,
                                ..options
                            },
                            self.pid,
                            self.prefer_raw_events,
                        ),
                    )?;
                    return Ok(Box::new(driver));
                }
            } else if #[cfg(target_os="macos")] {
                if self.kind == DriverKind::Default || self.kind == DriverKind::KPerf {
                    return Ok(Box::new(KPerfSamplingDriver::new(
                        &self.counters,
                        self.sample_freq,
                        self.pid,
                    )?));
                }
            }
        }

        Err(Error::UnsupportedDriver {
            driver: format!("{:?}", self.kind),
        })
    }
}

#[cfg(target_os = "linux")]
fn sampling_with_fallback<T, F>(
    mut counters: Vec<Counter>,
    branch_records: bool,
    mut open: F,
) -> Result<T, Error>
where
    F: FnMut(&[Counter], Option<perf::branch::BranchMode>) -> Result<T, Error>,
{
    let mut modes = if branch_records {
        perf::branch::BranchMode::LADDER.iter()
    } else {
        [].iter()
    };
    let mut branch_mode = modes.next().copied();

    loop {
        match open(&counters, branch_mode) {
            Ok(driver) => return Ok(driver),
            // An event this PMU simply does not implement says nothing about
            // branch records: drop the event, not the fidelity rung.
            Err(error)
                if error.is_event_unsupported()
                    && error.counter_name() != Some(Counter::Cycles.name()) =>
            {
                let unsupported = error.counter_name().unwrap_or_default();
                let Some(index) = counters
                    .iter()
                    .position(|counter| counter.name() == unsupported)
                else {
                    return Err(error);
                };
                counters.remove(index);
            }
            // Opening the event is the authoritative support probe for a
            // branch recorder: Intel LBR takes call-stack mode, AMD LbrV2 only
            // call filtering, AMD BRS only a plain history, and a VM none at
            // all. Step down the modes before touching the counters.
            Err(_) if branch_mode.is_some() => branch_mode = modes.next().copied(),
            Err(error) if error.counter_name() == Some(Counter::Cycles.name()) => {
                counters.retain(Counter::is_software);
                if !counters.contains(&Counter::CpuClock) {
                    counters.insert(0, Counter::CpuClock);
                }
            }
            Err(error) => return Err(error),
        }

        if counters.is_empty() {
            return Err(Error::InvalidConfiguration(
                "no sampling counters are available".to_owned(),
            ));
        }
    }
}

impl Default for SamplingDriverBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CounterResult {
    /// Constructs a result from individual counter entries.
    pub fn from_entries(entries: SmallVec<[CounterEntry; 16]>) -> Self {
        CounterResult { entries }
    }

    /// Faithful total for a counter, summed across every core it was measured
    /// on. On a homogeneous system this is simply the single value.
    pub fn get(&self, kind: Counter) -> Option<CounterValue> {
        let matching: SmallVec<[&CounterEntry; 8]> =
            self.entries.iter().filter(|e| e.counter == kind).collect();

        if matching.is_empty() {
            return None;
        }

        let value = matching.iter().map(|e| e.value.value).sum();
        let scaling = matching.iter().map(|e| e.value.scaling).sum::<f64>() / matching.len() as f64;

        Some(CounterValue {
            value,
            scaling,
            quality: MeasurementQuality::Exact,
        })
    }

    /// Value of a counter on one specific core.
    pub fn get_for(&self, core: &Option<CoreId>, kind: Counter) -> Option<CounterValue> {
        self.entries
            .iter()
            .find(|e| e.core == *core && e.counter == kind)
            .map(|e| e.value.clone())
    }

    /// The distinct cores present, in first-seen order. Empty on homogeneous
    /// systems (all entries are untagged).
    pub fn cores(&self) -> Vec<CoreId> {
        let mut cores: Vec<CoreId> = Vec::new();
        for entry in &self.entries {
            if let Some(core) = &entry.core {
                if !cores.contains(core) {
                    cores.push(core.clone());
                }
            }
        }
        cores
    }

    /// Returns all counter entries in collection order.
    pub fn entries(&self) -> &[CounterEntry] {
        &self.entries
    }
}

impl IntoIterator for CounterResult {
    type Item = CounterEntry;

    type IntoIter = <SmallVec<[CounterEntry; 16]> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_event_does_not_cost_the_branch_records() {
        let mut attempts = Vec::new();
        let selected = sampling_with_fallback(
            vec![
                Counter::Cycles,
                Counter::StalledCyclesBackend,
                Counter::CpuClock,
            ],
            true,
            |counters, branch_mode| {
                attempts.push((counters.to_vec(), branch_mode));
                if counters.contains(&Counter::StalledCyclesBackend) {
                    Err(Error::perf_event_open_with(
                        &Counter::StalledCyclesBackend,
                        None,
                        std::io::Error::from_raw_os_error(libc::ENOENT),
                        Some(4),
                    ))
                } else {
                    Ok(counters.to_vec())
                }
            },
        )
        .expect("dropping the unsupported event should open");

        assert_eq!(selected, vec![Counter::Cycles, Counter::CpuClock]);
        assert!(
            attempts
                .iter()
                .all(|(_, mode)| *mode == Some(perf::branch::BranchMode::CallStack)),
            "branch records must survive an unrelated counter failure"
        );
    }

    #[test]
    fn sampling_falls_back_to_cpu_clock_when_cycles_cannot_open() {
        let mut attempts = Vec::new();
        let selected = sampling_with_fallback(
            vec![Counter::Cycles, Counter::Instructions],
            false,
            |counters, _| {
                attempts.push(counters.to_vec());
                if counters.contains(&Counter::Cycles) {
                    Err(Error::perf_event_open_with(
                        &Counter::Cycles,
                        None,
                        std::io::Error::from_raw_os_error(libc::ENOENT),
                        Some(4),
                    ))
                } else {
                    Ok(counters.to_vec())
                }
            },
        )
        .expect("software fallback should open");

        assert_eq!(attempts.len(), 2);
        assert_eq!(
            selected,
            vec![Counter::CpuClock],
            "hardware-only sampling must become a cpu-clock-only group"
        );
    }
}
