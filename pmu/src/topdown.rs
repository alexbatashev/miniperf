use pmu_data::{TmaConstant, TmaGroup, TmaMetric, TmaScenario};

use crate::{Capabilities, Rung};

/// Intel PERF_METRICS events. The names are the sysfs aliases with `-` turned
/// into `_` so they are valid formula variables and SQL column names;
/// [`sysfs_alias`] maps them back.
pub(crate) const INTEL_EVENTS: [&str; 5] = [
    "slots",
    "topdown_retiring",
    "topdown_bad_spec",
    "topdown_fe_bound",
    "topdown_be_bound",
];

/// Sapphire-Rapids and newer report a second PERF_METRICS level: one half of
/// each level-one bucket, from which the other half follows by subtraction.
/// `(parent, parent event, counted half, its event, remaining half)`.
const INTEL_LEVEL_TWO: [(&str, &str, &str, &str, &str); 4] = [
    (
        "retiring",
        "topdown_retiring",
        "heavy_operations",
        "topdown_heavy_ops",
        "light_operations",
    ),
    (
        "bad_speculation",
        "topdown_bad_spec",
        "branch_mispredict",
        "topdown_br_mispredict",
        "machine_clears",
    ),
    (
        "fe_bound",
        "topdown_fe_bound",
        "fetch_latency",
        "topdown_fetch_lat",
        "fetch_bandwidth",
    ),
    (
        "be_bound",
        "topdown_be_bound",
        "memory_bound",
        "topdown_mem_bound",
        "core_bound",
    ),
];

/// Architected pmuv3 events of Arm's slots-based level-one methodology.
pub(crate) const ARM_EVENTS: [&str; 5] = [
    "stall_slot_frontend",
    "stall_slot_backend",
    "op_retired",
    "op_spec",
    "br_mis_pred_retired",
];

/// Whether an event belongs to a fixed-topdown group. Such events are opened
/// from the PMU's own sysfs alias and are never split across counter groups:
/// their ratios only mean something inside one scheduling domain.
pub fn is_topdown_event(name: &str) -> bool {
    INTEL_EVENTS.contains(&name)
        || ARM_EVENTS.contains(&name)
        || INTEL_LEVEL_TWO.iter().any(|level| level.3 == name)
}

/// Whether an event is an Intel PERF_METRICS event. The kernel refuses to open
/// these unless `slots` leads the group, and refuses to let either sample.
pub fn is_perf_metrics_event(name: &str) -> bool {
    name.starts_with("topdown_")
}

/// The sysfs `events/` alias backing a topdown event name.
pub fn sysfs_alias(name: &str) -> String {
    if is_perf_metrics_event(name) {
        name.replace('_', "-")
    } else {
        name.to_owned()
    }
}

/// The name of the event that must lead the group, when the hardware demands
/// one. Intel PERF_METRICS requires `slots`; Arm has no such constraint.
pub const GROUP_LEADER: &str = "slots";

/// Whether the kernel will actually schedule this PMU's topdown group.
///
/// No interface advertises how many counters a PMU has, and a group wider than
/// the hardware is rejected outright at `perf_event_open`. Opening the group
/// once is therefore the only honest test, and it keeps a narrow PMU degrading
/// down the ladder instead of failing the recording.
#[cfg(target_os = "linux")]
pub(crate) fn group_opens(pmu: &crate::PmuDevice, events: &[&str]) -> bool {
    use perf_event_open_sys as sys;

    let path = std::path::Path::new("/sys/bus/event_source/devices").join(&pmu.name);
    let mut fds = Vec::with_capacity(events.len());
    let mut opened_all = true;
    for event in events {
        // No encoding means the host cannot tell us anything about this group;
        // an absent event is already caught by the rung's own event check.
        let Some(mut attr) = crate::driver::perf::sysfs::event_attr(&path, event) else {
            continue;
        };
        attr.size = std::mem::size_of::<sys::bindings::perf_event_attr>() as u32;
        attr.set_disabled(fds.is_empty().into());
        attr.set_exclude_kernel(1);
        attr.set_exclude_hv(1);
        let leader = fds.first().copied().unwrap_or(-1);
        let fd = unsafe { sys::perf_event_open(&mut attr, 0, -1, leader, 0) };
        if fd < 0 {
            opened_all = false;
            break;
        }
        fds.push(fd);
    }
    for fd in fds {
        unsafe { libc::close(fd) };
    }
    opened_all
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn group_opens(_pmu: &crate::PmuDevice, _events: &[&str]) -> bool {
    false
}

/// The events a PMU's topdown group opens, leader first.
pub(crate) fn group_events(pmu: &crate::PmuDevice) -> Vec<String> {
    if pmu.name.starts_with("armv8_pmuv3") {
        ["cpu_cycles", "inst_retired"]
            .into_iter()
            .chain(ARM_EVENTS)
            .map(str::to_owned)
            .collect()
    } else {
        INTEL_EVENTS
            .iter()
            .map(|event| sysfs_alias(event))
            .chain(["cpu-cycles".to_owned(), "instructions".to_owned()])
            .collect()
    }
}

/// The fixed-topdown scenario this host supports, if any. Returns `None` when
/// the level-one breakdown has to be estimated from event arithmetic instead.
pub fn scenario(caps: &Capabilities) -> Option<TmaScenario> {
    if Rung::FixedTopdown.rejection(caps).is_none() {
        let level_two = caps.core_pmus().any(|pmu| {
            INTEL_LEVEL_TWO
                .iter()
                .all(|level| pmu.has_event(&sysfs_alias(level.3)))
        });
        return Some(intel_scenario(level_two));
    }
    if Rung::ArmSlotsTopdown.rejection(caps).is_none() {
        return arm_scenario(caps);
    }
    None
}

fn intel_scenario(level_two: bool) -> TmaScenario {
    let level_two_events = level_two
        .then_some(INTEL_LEVEL_TWO)
        .into_iter()
        .flatten()
        .map(|level| level.3);
    let events = ["cycles", "instructions"]
        .into_iter()
        .chain(INTEL_EVENTS)
        .chain(level_two_events)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut metrics: Vec<TmaMetric> = [
        ("retiring", "Slots retiring useful work", "topdown_retiring"),
        (
            "bad_speculation",
            "Slots wasted on mispredicted or cancelled work",
            "topdown_bad_spec",
        ),
        (
            "fe_bound",
            "Slots the frontend failed to deliver",
            "topdown_fe_bound",
        ),
        (
            "be_bound",
            "Slots the backend could not accept",
            "topdown_be_bound",
        ),
    ]
    .into_iter()
    .map(|(name, desc, event)| TmaMetric {
        name: name.to_owned(),
        desc: desc.to_owned(),
        formula: format!("{event} / slots"),
        group: Some("topdown".to_owned()),
        cpus: None,
    })
    .collect();

    if level_two {
        for (parent, parent_event, half, half_event, rest) in INTEL_LEVEL_TWO {
            metrics.push(TmaMetric {
                name: format!("{parent}.{half}"),
                desc: format!("Part of {parent} counted directly by PERF_METRICS"),
                formula: format!("{half_event} / slots"),
                group: Some("topdown".to_owned()),
                cpus: None,
            });
            metrics.push(TmaMetric {
                name: format!("{parent}.{rest}"),
                desc: format!("Remainder of {parent} once {half} is removed"),
                formula: format!("({parent_event} - {half_event}) / slots"),
                group: Some("topdown".to_owned()),
                cpus: None,
            });
        }
    }

    TmaScenario {
        name: "tma".to_owned(),
        groups: vec![TmaGroup {
            name: "topdown".to_owned(),
            events: events.clone(),
        }],
        events,
        precise_attribution: false,
        constants: Vec::new(),
        metrics,
        ui: None,
    }
}

/// Arm's level-one topdown from the architected slots events. On a
/// heterogeneous host each core type has its own issue width, so it also gets
/// its own metric set, restricted to that cluster's CPUs.
fn arm_scenario(caps: &Capabilities) -> Option<TmaScenario> {
    let clusters = arm_clusters(caps);
    if clusters.is_empty() {
        return None;
    }

    let events = ["cycles", "instructions"]
        .into_iter()
        .chain(ARM_EVENTS)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let hybrid = clusters.len() > 1;
    let mut constants = Vec::new();
    let mut metrics = Vec::new();
    for cluster in &clusters {
        let constant = format!("slots_{}", cluster.name);
        constants.push(TmaConstant {
            name: constant.clone(),
            value: cluster.slots,
        });
        for (name, desc, formula) in arm_metrics(&constant) {
            metrics.push(TmaMetric {
                name: if hybrid {
                    format!("{name}.{}", cluster.name)
                } else {
                    name.to_owned()
                },
                desc: desc.to_owned(),
                formula,
                group: Some("topdown".to_owned()),
                cpus: hybrid.then(|| cluster.cpus.clone()),
            });
        }
    }

    Some(TmaScenario {
        name: "tma".to_owned(),
        groups: vec![TmaGroup {
            name: "topdown".to_owned(),
            events: events.clone(),
        }],
        events,
        precise_attribution: false,
        constants,
        metrics,
        ui: None,
    })
}

/// Arm's topdown-L1 formulas, with the branch-misprediction term moved out of
/// the frontend and into bad speculation symmetrically so the four fractions
/// still add up to one.
fn arm_metrics(slots: &str) -> [(&'static str, &'static str, String); 4] {
    let issued = format!("(${slots} * cycles)");
    let stalled = format!("((stall_slot_frontend + stall_slot_backend) / {issued})");
    let mispredict = format!("(br_mis_pred_retired / {issued})");
    [
        (
            "retiring",
            "Slots retiring useful work",
            format!("(op_retired / op_spec) * (1 - {stalled})"),
        ),
        (
            "bad_speculation",
            "Slots wasted on mispredicted or cancelled work",
            format!("(1 - op_retired / op_spec) * (1 - {stalled}) + {mispredict}"),
        ),
        (
            "fe_bound",
            "Slots the frontend failed to deliver",
            format!("stall_slot_frontend / {issued} - {mispredict}"),
        ),
        (
            "be_bound",
            "Slots the backend could not accept",
            format!("stall_slot_backend / {issued}"),
        ),
    ]
}

struct ArmCluster {
    name: String,
    cpus: String,
    slots: u32,
}

/// Pair every pmuv3 instance that advertises a `slots` capability with the CPUs
/// it covers. One entry means a homogeneous host.
fn arm_clusters(caps: &Capabilities) -> Vec<ArmCluster> {
    let cores = crate::host_core_clusters();
    caps.pmus_with_prefix("armv8_pmuv3")
        .filter_map(|pmu| {
            let slots = pmu.cap_number("slots")? as u32;
            let cpus = pmu.cpus.clone().unwrap_or_default();
            let core = cores.iter().find(|core| core.cpus == cpus);
            Some(ArmCluster {
                name: core.map_or_else(|| pmu.name.clone(), |core| core.family_id.clone()),
                cpus,
                slots,
            })
        })
        .filter(|cluster| cluster.slots > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PmuDevice;

    fn intel_host() -> Capabilities {
        let mut cpu = PmuDevice {
            name: "cpu".to_owned(),
            ..PmuDevice::default()
        };
        for event in INTEL_EVENTS {
            cpu.events.insert(sysfs_alias(event));
        }
        Capabilities {
            pmus: vec![cpu],
            ..Capabilities::default()
        }
    }

    #[test]
    fn intel_metrics_are_plain_fractions_of_slots() {
        let scenario = scenario(&intel_host()).expect("fixed topdown");
        let formulas = scenario
            .metrics
            .iter()
            .map(|metric| (metric.name.as_str(), metric.formula.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            formulas,
            vec![
                ("retiring", "topdown_retiring / slots"),
                ("bad_speculation", "topdown_bad_spec / slots"),
                ("fe_bound", "topdown_fe_bound / slots"),
                ("be_bound", "topdown_be_bound / slots"),
            ]
        );
        assert!(scenario.metrics.iter().all(|metric| metric.cpus.is_none()));
        assert_eq!(scenario.groups.len(), 1);
    }

    #[test]
    fn arm_level_one_fractions_add_up_to_one() {
        let slots = 5.0_f64;
        let cycles = 1_000_000.0;
        let issued = slots * cycles;
        let front = 200_000.0;
        let back = 1_500_000.0;
        let mispredict = 3_000.0;
        let retired = 900_000.0;
        let speculated = 1_000_000.0;

        let stalled = (front + back) / issued;
        let retiring = (retired / speculated) * (1.0 - stalled);
        let bad = (1.0 - retired / speculated) * (1.0 - stalled) + mispredict / issued;
        let fe = front / issued - mispredict / issued;
        let be = back / issued;

        assert!((retiring + bad + fe + be - 1.0).abs() < 1e-12);
    }

    #[test]
    fn level_two_metrics_appear_only_when_the_pmu_counts_them() {
        let mut caps = intel_host();
        assert!(scenario(&caps)
            .expect("fixed topdown")
            .metrics
            .iter()
            .all(|metric| !metric.name.contains('.')));

        for level in INTEL_LEVEL_TWO {
            caps.pmus[0].events.insert(sysfs_alias(level.3));
        }
        let scenario = scenario(&caps).expect("fixed topdown");
        assert_eq!(scenario.metrics.len(), 12);
        assert_eq!(scenario.metrics[4].name, "retiring.heavy_operations");
        assert_eq!(
            scenario.metrics[5].formula,
            "(topdown_retiring - topdown_heavy_ops) / slots"
        );
    }

    #[test]
    fn heterogeneous_hosts_get_one_metric_set_per_core_type() {
        let cluster = |name: &str, cpus: &str, slots: &str| {
            let mut pmu = PmuDevice {
                name: name.to_owned(),
                cpus: Some(cpus.to_owned()),
                ..PmuDevice::default()
            };
            pmu.caps.insert("slots".to_owned(), slots.to_owned());
            pmu.events.extend(ARM_EVENTS.iter().map(|e| e.to_string()));
            pmu
        };
        let caps = Capabilities {
            pmus: vec![
                cluster("armv8_pmuv3_0", "0,5-11", "0x00000005"),
                cluster("armv8_pmuv3_1", "1-4", "0x00000003"),
            ],
            ..Capabilities::default()
        };

        let scenario = scenario(&caps).expect("arm slots topdown");
        assert_eq!(scenario.metrics.len(), 8);
        assert_eq!(scenario.metrics[0].name, "retiring.armv8_pmuv3_0");
        assert_eq!(scenario.metrics[0].cpus.as_deref(), Some("0,5-11"));
        assert_eq!(scenario.metrics[4].name, "retiring.armv8_pmuv3_1");
        assert_eq!(scenario.metrics[4].cpus.as_deref(), Some("1-4"));
        assert!(scenario.metrics[0].formula.contains("$slots_armv8_pmuv3_0"));
        assert_eq!(
            scenario
                .constants
                .iter()
                .map(|constant| constant.value)
                .collect::<Vec<_>>(),
            vec![5, 3]
        );
    }

    #[test]
    fn topdown_event_names_map_to_their_sysfs_aliases() {
        assert_eq!(sysfs_alias("topdown_bad_spec"), "topdown-bad-spec");
        assert_eq!(sysfs_alias("slots"), "slots");
        assert_eq!(sysfs_alias("stall_slot_frontend"), "stall_slot_frontend");
        assert!(is_topdown_event("op_spec"));
        assert!(!is_topdown_event("cycles"));
    }
}
