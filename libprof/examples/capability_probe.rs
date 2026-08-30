//! Print what this host can capture: PMUs, host facilities, and the mechanism
//! each requestable feature resolves to.

use libprof::Feature;

fn main() {
    let caps = libprof::capabilities();
    println!(
        "kernel {}  paranoid {}  cap_perfmon {}  root {}",
        caps.kernel_version.as_deref().unwrap_or("unknown"),
        caps.perf_event_paranoid
            .map_or_else(|| "unknown".to_string(), |level| level.to_string()),
        caps.has_cap_perfmon,
        caps.is_root
    );
    if let Some(precise) = caps.max_precise() {
        println!("core PMU max_precise {precise}");
    }
    for pmu in &caps.pmus {
        println!(
            "  {:<16} type {:<6} {} events, {} formats, caps {:?}",
            pmu.name,
            pmu.type_id
                .map_or_else(|| "-".to_string(), |ty| ty.to_string()),
            pmu.events.len(),
            pmu.formats.len(),
            pmu.caps
        );
    }
    for feature in [
        Feature::PreciseMem,
        Feature::Topdown,
        Feature::HwCallstack,
        Feature::DramBw,
    ] {
        let resolution = libprof::resolve(feature, &caps);
        match resolution.satisfied {
            Some(satisfied) => println!(
                "  {:<14} {} ({:?})",
                feature.name(),
                satisfied.mechanism.name(),
                satisfied.quality
            ),
            None => println!("  {:<14} unavailable", feature.name()),
        }
        for (mechanism, reason) in &resolution.rejected {
            println!("    {:<16} {reason}", mechanism.name());
        }
    }
}
