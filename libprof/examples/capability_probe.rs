//! Print what this host can capture: PMUs, host facilities, and the rung each
//! capture strategy resolves to.

use libprof::Rung;

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
    for rung in [
        Rung::PebsMem,
        Rung::IbsOp,
        Rung::ArmSpe,
        Rung::FixedTopdown,
        Rung::ArmSlotsTopdown,
        Rung::LbrCallstack,
        Rung::UncoreBw,
        Rung::Baseline,
    ] {
        match rung.rejection(&caps) {
            None => println!("  {:<18} available", rung.name()),
            Some(reason) => println!("  {:<18} {reason}", rung.name()),
        }
    }
}
