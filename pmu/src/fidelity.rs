use crate::Capabilities;

/// A capture strategy a scenario can run at. Scenarios declare an ordered
/// ladder of these, best first; [`resolve`] picks the best rung the host
/// supports and explains every rung it had to reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rung {
    /// Intel PEBS precise memory sampling (address, data source, latency).
    PebsMem,
    /// AMD IBS op sampling through the `ibs_op` PMU.
    IbsOp,
    /// Arm Statistical Profiling Extension through an `arm_spe_*` PMU.
    ArmSpe,
    /// Intel fixed topdown (`slots` + `topdown-*` PERF_METRICS).
    FixedTopdown,
    /// Arm pmuv3 slots-based L1 topdown.
    ArmSlotsTopdown,
    /// Intel LBR call-stack mode as a frame-pointer-free stack source.
    LbrCallstack,
    /// Memory-controller counters for measured DRAM bandwidth.
    UncoreBw,
    /// Plain counters and frame-pointer/DWARF stacks: always available.
    Baseline,
}

impl Rung {
    /// Stable identifier written to the session and shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            Rung::PebsMem => "pebs_mem",
            Rung::IbsOp => "ibs_op",
            Rung::ArmSpe => "arm_spe",
            Rung::FixedTopdown => "fixed_topdown",
            Rung::ArmSlotsTopdown => "arm_slots_topdown",
            Rung::LbrCallstack => "lbr_callstack",
            Rung::UncoreBw => "uncore_bw",
            Rung::Baseline => "counter_only",
        }
    }

    /// Why this rung cannot run on this host, or `None` when it can.
    pub fn rejection(self, caps: &Capabilities) -> Option<String> {
        match self {
            Rung::PebsMem => {
                if caps.core_pmus().next().is_none() {
                    return Some("PEBS: no core PMU exposed in sysfs".to_string());
                }
                if !caps.max_precise().is_some_and(|precise| precise >= 2) {
                    return Some(format!(
                        "PEBS: core PMU advertises max_precise={} — precise sampling needs 2",
                        caps.max_precise().unwrap_or(0)
                    ));
                }
                let missing: Vec<&str> = ["mem-loads", "mem-stores"]
                    .into_iter()
                    .filter(|event| !caps.core_pmus().any(|pmu| pmu.has_event(event)))
                    .collect();
                if !missing.is_empty() {
                    return Some(format!(
                        "PEBS: core PMU exposes no {} event alias",
                        missing.join("/")
                    ));
                }
                // PERF_SAMPLE_WEIGHT_STRUCT, which carries the access latency,
                // landed in 5.12; without it a sample says where but not how bad.
                let release = caps.kernel_version.as_deref().unwrap_or_default();
                if kernel_at_least(release, 5, 12) == Some(false) {
                    return Some(format!(
                        "PEBS: kernel {release} predates 5.12 — PERF_SAMPLE_WEIGHT_STRUCT is unavailable"
                    ));
                }
                None
            }
            Rung::IbsOp => {
                if caps.pmu("ibs_op").is_some() {
                    return None;
                }
                if cfg!(target_arch = "x86_64") && !caps.has_cpu_flag("ibs") {
                    return Some(
                        "IBS: `ibs` CPUID flag absent — possibly disabled in BIOS".to_string(),
                    );
                }
                Some("IBS: no `ibs_op` PMU exposed by the kernel".to_string())
            }
            Rung::ArmSpe => (caps.pmus_with_prefix("arm_spe").next().is_none()).then(|| {
                "Arm SPE: no `arm_spe_*` PMU exposed — needs CONFIG_ARM_SPE_PMU and firmware support"
                    .to_string()
            }),
            // A hybrid host needs one topdown group per core type, opened only
            // on that type's CPUs; until that exists, degrade rather than open
            // a P-core group on an E-core and fail the recording.
            Rung::FixedTopdown if caps.pmu("cpu_atom").is_some() => Some(
                "fixed topdown: hybrid Intel cores (cpu_core/cpu_atom) are not supported yet"
                    .to_string(),
            ),
            Rung::FixedTopdown => {
                let Some(pmu) = caps.core_pmus().find(|pmu| {
                    crate::topdown::INTEL_EVENTS
                        .iter()
                        .all(|event| pmu.has_event(&crate::sysfs_alias(event)))
                }) else {
                    return Some(
                        "fixed topdown: core PMU exposes no `slots` + `topdown-*` events (PERF_METRICS is Icelake and newer)"
                            .to_string(),
                    );
                };
                group_is_schedulable(pmu)
            }
            Rung::ArmSlotsTopdown => {
                let pmuv3: Vec<_> = caps.pmus_with_prefix("armv8_pmuv3").collect();
                if pmuv3.is_empty() {
                    return Some("Arm topdown: no `armv8_pmuv3*` PMU exposed".to_string());
                }
                // Every cluster must be measurable: a per-core-type breakdown
                // that silently omits one core type is worse than none.
                if pmuv3.iter().any(|pmu| pmu.cap_number("slots").is_none()) {
                    return Some(
                        "Arm topdown: pmuv3 advertises no `slots` capability".to_string(),
                    );
                }
                if pmuv3.iter().any(|pmu| {
                    !crate::topdown::ARM_EVENTS
                        .iter()
                        .all(|event| pmu.has_event(event))
                }) {
                    return Some(format!(
                        "Arm topdown: a pmuv3 instance is missing the architected slots events ({})",
                        crate::topdown::ARM_EVENTS.join("/")
                    ));
                }
                pmuv3.iter().find_map(|pmu| group_is_schedulable(pmu))
            }
            Rung::LbrCallstack => {
                let depth = caps
                    .core_pmus()
                    .filter_map(|pmu| pmu.cap_number("branches"))
                    .max();
                (!depth.is_some_and(|branches| branches > 0)).then(|| {
                    "LBR: core PMU advertises no branch-record depth in `caps/branches`".to_string()
                })
            }
            Rung::UncoreBw => {
                if !caps.system_wide_allowed() {
                    return Some(format!(
                        "uncore bandwidth: system-wide events need CAP_PERFMON or perf_event_paranoid <= 0 (currently {})",
                        caps.perf_event_paranoid
                            .map_or_else(|| "unknown".to_string(), |level| level.to_string())
                    ));
                }
                (!crate::platform_memory::bandwidth_counters_present(caps)).then(|| {
                    "uncore bandwidth: no memory-controller PMU exposed (uncore_imc*/amd_df/amd_umc/arm_cmn)"
                        .to_string()
                })
            }
            Rung::Baseline => None,
        }
    }
}

/// Reject a topdown rung whose group the PMU is too narrow to schedule. The
/// event set is fixed by the methodology, so a PMU that cannot hold it has to
/// degrade to the arithmetic baseline rather than fail the recording.
fn group_is_schedulable(pmu: &crate::PmuDevice) -> Option<String> {
    let events = crate::topdown::group_events(pmu);
    let events = events.iter().map(String::as_str).collect::<Vec<_>>();
    (!crate::topdown::group_opens(pmu, &events)).then(|| {
        format!(
            "topdown: {} cannot schedule the whole group ({}) in one counter set",
            pmu.name,
            events.join("/")
        )
    })
}

/// Whether a `uname -r` release is at least `major.minor`. `None` when the
/// string cannot be parsed, so an unreadable version never blocks a rung.
fn kernel_at_least(release: &str, major: u32, minor: u32) -> Option<bool> {
    let mut parts = release.split(|c: char| !c.is_ascii_digit());
    let found_major = parts.next()?.parse::<u32>().ok()?;
    let found_minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some((found_major, found_minor) >= (major, minor))
}

/// The rung a scenario runs at, plus every better rung that was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The best rung this host supports.
    pub chosen: Rung,
    /// Rejected rungs, best first, each with a human-readable reason.
    pub rejected: Vec<(Rung, String)>,
}

/// Pick the best rung in `ladder` that `caps` supports. Ladders always end at
/// [`Rung::Baseline`], which every host satisfies.
pub fn resolve(ladder: &[Rung], caps: &Capabilities) -> Resolution {
    let mut rejected = Vec::new();
    for rung in ladder {
        match rung.rejection(caps) {
            None => {
                return Resolution {
                    chosen: *rung,
                    rejected,
                };
            }
            Some(reason) => rejected.push((*rung, reason)),
        }
    }
    Resolution {
        chosen: Rung::Baseline,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PmuDevice;

    fn pmu(name: &str) -> PmuDevice {
        PmuDevice {
            name: name.to_string(),
            ..PmuDevice::default()
        }
    }

    #[test]
    fn empty_host_falls_back_to_baseline_with_reasons() {
        let caps = Capabilities::default();
        let resolution = resolve(&[Rung::PebsMem, Rung::IbsOp, Rung::Baseline], &caps);
        assert_eq!(resolution.chosen, Rung::Baseline);
        assert_eq!(resolution.rejected.len(), 2);
        assert!(resolution.rejected[0].1.contains("no core PMU"));
    }

    #[test]
    fn reports_missing_ibs_cpuid_flag() {
        let caps = Capabilities {
            pmus: vec![pmu("cpu")],
            ..Capabilities::default()
        };
        let reason = Rung::IbsOp.rejection(&caps).unwrap();
        if cfg!(target_arch = "x86_64") {
            assert!(reason.contains("CPUID flag absent"), "{reason}");
        }
    }

    #[test]
    fn picks_the_best_satisfied_rung() {
        let caps = Capabilities {
            pmus: vec![pmu("ibs_op")],
            ..Capabilities::default()
        };
        let resolution = resolve(&[Rung::PebsMem, Rung::IbsOp, Rung::Baseline], &caps);
        assert_eq!(resolution.chosen, Rung::IbsOp);
        assert_eq!(resolution.rejected.len(), 1);
        assert_eq!(resolution.rejected[0].0, Rung::PebsMem);
    }

    #[test]
    fn parses_kernel_releases() {
        assert_eq!(kernel_at_least("5.11.0-generic", 5, 12), Some(false));
        assert_eq!(kernel_at_least("5.12.0", 5, 12), Some(true));
        assert_eq!(kernel_at_least("7.0.0-29-generic", 5, 12), Some(true));
        assert_eq!(kernel_at_least("unknown", 5, 12), None);
    }

    #[test]
    fn uncore_requires_system_wide_access() {
        let caps = Capabilities {
            pmus: vec![pmu("uncore_imc_0")],
            perf_event_paranoid: Some(2),
            ..Capabilities::default()
        };
        assert!(Rung::UncoreBw
            .rejection(&caps)
            .unwrap()
            .contains("CAP_PERFMON"));

        let allowed = Capabilities {
            perf_event_paranoid: Some(-1),
            ..caps
        };
        assert_eq!(Rung::UncoreBw.rejection(&allowed), None);
    }
}
