use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use mperf_data::{Scenario, SnapshotCollectorStatus};
use pmu::{Counter, Process};

use crate::event_dispatcher::EventDispatcher;

/// What a source provides, declared before probing.
pub struct SourceDecl {
    pub name: &'static str,
    /// Logical session tables this source writes.
    #[allow(dead_code)]
    pub provides: &'static [&'static str],
}

/// Result of probing whether a source can run here.
pub enum Availability {
    Available,
    Unavailable { reason: String },
}

/// Everything a source needs to know about the pass it runs in. The target
/// process is the launched (still suspended) root, or an attach PID.
pub struct SessionContext {
    pub directory: PathBuf,
    pub dispatcher: Arc<EventDispatcher>,
    pub process: Option<Arc<Process>>,
    pub attached_pid: Option<u32>,
}

impl SessionContext {
    pub fn root_pid(&self) -> u32 {
        self.attached_pid
            .unwrap_or_else(|| self.process.as_ref().expect("no target process").pid() as u32)
    }

    pub fn launched(&self) -> bool {
        self.process.is_some()
    }
}

/// Per-source outcome recorded into the session; generalizes the snapshot
/// collector status and feeds `ScenarioInfo` warnings.
pub type SourceStatus = SnapshotCollectorStatus;

/// A data producer contributing tables to the session directory. Scenarios
/// declare required and optional sources per pass; the orchestrator resolves
/// the declarations against probed availability, injects child environment
/// before launch, and records per-source status.
pub trait Source {
    fn declare(&self) -> SourceDecl;

    fn as_any(&self) -> &dyn std::any::Any;

    fn probe(&self, directory: &Path) -> Availability;

    /// Environment the profiled child needs for this source, injected before
    /// the process is created.
    fn child_environment(&self, _directory: &Path) -> Vec<(String, String)> {
        Vec::new()
    }

    fn start(&mut self, context: &SessionContext) -> Result<()>;

    fn stop(&mut self, context: &SessionContext) -> Vec<SourceStatus>;
}

/// One app execution with its own source set. A scenario is an ordered list
/// of passes; single-pass scenarios are the one-element case. Passes share
/// the session directory: per-process file names make merge concatenation
/// and hash-based identities give cross-pass correlation for free.
pub struct Pass {
    pub name: &'static str,
    pub required: Vec<Box<dyn Source>>,
    pub optional: Vec<Box<dyn Source>>,
}

/// Sources selected for one pass, plus statuses of everything declared but
/// not running.
pub struct ResolvedPass {
    pub name: &'static str,
    pub sources: Vec<Box<dyn Source>>,
    pub statuses: Vec<SourceStatus>,
}

impl Pass {
    /// Resolve declarations against probed availability: every required
    /// source must be available; optional sources degrade to a status entry.
    pub fn resolve(self, directory: &Path) -> Result<ResolvedPass> {
        let mut sources = Vec::new();
        let mut statuses = Vec::new();
        for source in self.required {
            match source.probe(directory) {
                Availability::Available => sources.push(source),
                Availability::Unavailable { reason } => anyhow::bail!(
                    "required source '{}' is unavailable: {reason}",
                    source.declare().name
                ),
            }
        }
        for source in self.optional {
            match source.probe(directory) {
                Availability::Available => sources.push(source),
                Availability::Unavailable { reason } => statuses.push(SourceStatus {
                    name: source.declare().name.to_string(),
                    status: "unavailable".to_string(),
                    source: "probe".to_string(),
                    quality: "missing".to_string(),
                    message: reason,
                }),
            }
        }
        Ok(ResolvedPass {
            name: self.name,
            sources,
            statuses,
        })
    }
}

impl ResolvedPass {
    pub fn child_environment(&self, directory: &Path) -> Vec<(String, String)> {
        self.sources
            .iter()
            .flat_map(|source| source.child_environment(directory))
            .collect()
    }

    pub fn start(&mut self, context: &SessionContext) -> Result<()> {
        for source in &mut self.sources {
            source.start(context).map_err(|error| {
                error.context(format!(
                    "failed to start source '{}' in pass '{}'",
                    source.declare().name,
                    self.name
                ))
            })?;
        }
        Ok(())
    }

    pub fn stop(&mut self, context: &SessionContext) -> Vec<SourceStatus> {
        let mut statuses = std::mem::take(&mut self.statuses);
        for source in &mut self.sources {
            statuses.extend(source.stop(context));
        }
        statuses
    }
}

/// PMU sampling as a source. The counter list is part of the declaration,
/// parameterized by scenario — this is where the old `counter_selection`
/// hardcoding lives now.
pub struct PmuSamplingSource {
    counters: Vec<Counter>,
    sample_freq: Option<u64>,
    stack_dump_size: Option<u32>,
    drivers: Vec<Box<dyn pmu::SamplingDriver>>,
    recorded: Vec<Counter>,
}

impl PmuSamplingSource {
    pub fn for_scenario(scenario: Scenario) -> PmuSamplingSource {
        let counters = scenario_counters(scenario);
        let (sample_freq, stack_dump_size) = match scenario {
            Scenario::Snapshot if cfg!(target_os = "linux") => (Some(99), Some(2 * 1024)),
            _ => (None, None),
        };
        PmuSamplingSource {
            counters,
            sample_freq,
            stack_dump_size,
            drivers: Vec::new(),
            recorded: Vec::new(),
        }
    }

    /// Counters the drivers actually opened, for `ScenarioInfo`.
    pub fn recorded_counters(&self) -> Vec<(mperf_data::EventType, String)> {
        self.recorded
            .iter()
            .map(|counter| {
                (
                    crate::utils::counter_to_event_ty(counter),
                    counter.name().to_string(),
                )
            })
            .collect()
    }
}

impl Source for PmuSamplingSource {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn declare(&self) -> SourceDecl {
        SourceDecl {
            name: "pmu_sampling",
            provides: &["samples", "stacks", "modules"],
        }
    }

    fn probe(&self, _directory: &Path) -> Availability {
        Availability::Available
    }

    fn start(&mut self, context: &SessionContext) -> Result<()> {
        let target_pids = if let Some(pid) = context.attached_pid {
            #[cfg(target_os = "linux")]
            {
                let tree = crate::snapshot_resources::process_tree(pid);
                if tree.is_empty() {
                    vec![pid]
                } else {
                    tree.into_iter().map(|member| member.pid).collect()
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                vec![pid]
            }
        } else {
            vec![context.root_pid()]
        };
        for target_pid in target_pids {
            let mut builder = pmu::SamplingDriverBuilder::new().counters(&self.counters);
            if let Some(freq) = self.sample_freq {
                builder = builder.sample_freq(freq);
            }
            #[cfg(target_os = "linux")]
            if let Some(size) = self.stack_dump_size {
                builder = builder.stack_dump_size(size);
            }
            if let Some(process) = &context.process {
                builder = builder.process(process);
            } else {
                builder = builder.pid(target_pid as i32);
            }
            self.drivers.push(builder.build()?);
        }
        if let Some(driver) = self.drivers.first() {
            self.recorded = driver.counters();
        }
        let callback = crate::record::sample_callback(context.dispatcher.clone());
        for driver in &mut self.drivers {
            driver.start(callback.clone())?;
        }
        Ok(())
    }

    fn stop(&mut self, _context: &SessionContext) -> Vec<SourceStatus> {
        let mut message = String::new();
        let mut status = "available";
        for driver in &mut self.drivers {
            if let Err(error) = driver.stop() {
                status = "degraded";
                message = error.to_string();
            }
        }
        self.drivers.clear();
        vec![SourceStatus {
            name: "pmu_sampling".to_string(),
            status: status.to_string(),
            source: "perf_events".to_string(),
            quality: if status == "available" {
                "exact"
            } else {
                "lossy"
            }
            .to_string(),
            message,
        }]
    }
}

/// The self-contained internal-event collector inside the profiled process:
/// activated purely through the child environment; contributes event tables
/// written by the child itself.
pub struct InternalEventsSource {
    pub roofline_instrumented: bool,
}

impl Source for InternalEventsSource {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn declare(&self) -> SourceDecl {
        SourceDecl {
            name: "internal_events",
            provides: &["events", "payloads", "strings", "clock"],
        }
    }

    fn probe(&self, _directory: &Path) -> Availability {
        Availability::Available
    }

    fn child_environment(&self, directory: &Path) -> Vec<(String, String)> {
        let directory = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_owned());
        let mut env = vec![(
            "MPERF_SESSION_DIR".to_string(),
            directory.to_string_lossy().into_owned(),
        )];
        if let Some(library) = collector_library_path() {
            env.push((
                "MPERF_COLLECTOR_LIBRARY".to_string(),
                library.to_string_lossy().into_owned(),
            ));
        }
        if self.roofline_instrumented {
            env.push((
                "MPERF_COLLECTOR_ROOFLINE_INSTRUMENTED".to_string(),
                "1".to_string(),
            ));
        }
        env
    }

    fn start(&mut self, _context: &SessionContext) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self, _context: &SessionContext) -> Vec<SourceStatus> {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
pub use linux_sources::{BpfSource, ProcfsSource};

#[cfg(target_os = "linux")]
mod linux_sources {
    use super::*;
    use crate::snapshot_resources::{BpfMonitor, CgroupScope, ResourceMonitor};

    /// procfs/cgroup resource pollers as one source (they share the private
    /// cgroup scope and the once-per-second collection loop).
    #[derive(Default)]
    pub struct ProcfsSource {
        cgroup: Option<CgroupScope>,
        monitor: Option<ResourceMonitor>,
    }

    impl Source for ProcfsSource {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn declare(&self) -> SourceDecl {
            SourceDecl {
                name: "procfs_resources",
                provides: &[
                    "snapshot_resource_samples",
                    "snapshot_processes",
                    "snapshot_collectors",
                ],
            }
        }

        fn probe(&self, _directory: &Path) -> Availability {
            Availability::Available
        }

        fn start(&mut self, context: &SessionContext) -> Result<()> {
            let cgroup = CgroupScope::create(context.root_pid(), context.launched());
            self.monitor = Some(ResourceMonitor::start(
                context.root_pid(),
                context.launched(),
                cgroup.path(),
                &context.directory,
            )?);
            self.cgroup = Some(cgroup);
            Ok(())
        }

        fn stop(&mut self, _context: &SessionContext) -> Vec<SourceStatus> {
            let mut statuses = self
                .monitor
                .take()
                .map(|monitor| monitor.stop())
                .unwrap_or_default();
            if let Some(cgroup) = self.cgroup.take() {
                statuses.push(cgroup.status());
            }
            statuses
        }
    }

    /// bpftrace scheduling/block-IO/network probes.
    #[derive(Default)]
    pub struct BpfSource {
        monitor: Option<BpfMonitor>,
    }

    impl Source for BpfSource {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn declare(&self) -> SourceDecl {
            SourceDecl {
                name: "bpf",
                provides: &["snapshot_bpf"],
            }
        }

        fn probe(&self, _directory: &Path) -> Availability {
            if !Path::new("/sys/kernel/btf/vmlinux").is_file() {
                return Availability::Unavailable {
                    reason: "kernel BTF is unavailable".to_string(),
                };
            }
            Availability::Available
        }

        fn start(&mut self, context: &SessionContext) -> Result<()> {
            self.monitor = Some(BpfMonitor::start(context.root_pid(), &context.directory));
            Ok(())
        }

        fn stop(&mut self, _context: &SessionContext) -> Vec<SourceStatus> {
            self.monitor
                .take()
                .map(|monitor| vec![monitor.stop()])
                .unwrap_or_default()
        }
    }
}

pub fn scenario_counters(scenario: Scenario) -> Vec<Counter> {
    match scenario {
        Scenario::Snapshot | Scenario::Mem | Scenario::Roofline => vec![
            Counter::Cycles,
            Counter::Instructions,
            Counter::LLCReferences,
            Counter::LLCMisses,
            Counter::BranchMisses,
            Counter::BranchInstructions,
            Counter::StalledCyclesBackend,
            Counter::StalledCyclesFrontend,
            Counter::CpuClock,
            Counter::CpuMigrations,
            Counter::PageFaults,
            Counter::ContextSwitches,
        ],
        Scenario::TMA => pmu::host_tma_scenario()
            .expect("TMA counter selection requires a supported host CPU")
            .events
            .iter()
            .map(|event| match event.as_str() {
                "cycles" => Counter::Cycles,
                "instructions" => Counter::Instructions,
                "stalled_cycles_frontend" => Counter::StalledCyclesFrontend,
                "stalled_cycles_backend" => Counter::StalledCyclesBackend,
                _ => Counter::Custom(event.clone()),
            })
            .chain([
                Counter::CpuClock,
                Counter::CpuMigrations,
                Counter::PageFaults,
                Counter::ContextSwitches,
            ])
            .collect(),
    }
}

/// The collector core shipped next to this binary, when present.
fn collector_library_path() -> Option<PathBuf> {
    let library = if cfg!(target_os = "macos") {
        "libmperf_collector.dylib"
    } else {
        "libmperf_collector.so"
    };
    let mut path = std::env::current_exe().ok()?;
    path.pop();
    for candidate in [path.join(library), path.join("../lib").join(library)] {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
