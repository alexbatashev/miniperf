use crate::{cpu_family, Counter};

pub fn list_supported_counters() -> Vec<Counter> {
    let mut counters = vec![
        Counter::Cycles,
        Counter::Instructions,
        Counter::BranchInstructions,
        Counter::BranchMisses,
        Counter::LLCMisses,
        Counter::LLCReferences,
        Counter::StalledCyclesFrontend,
        Counter::StalledCyclesBackend,
        Counter::CpuClock,
        Counter::PageFaults,
        Counter::CpuMigrations,
        Counter::ContextSwitches,
    ];

    let cpu_family = cpu_family::get_host_cpu_family();
    let events = cpu_family::find_cpu_family(cpu_family);

    if let Some(events) = events {
        for evt in events.events.values() {
            counters.push(Counter::Internal {
                name: evt.name.clone(),
                desc: evt.desc.clone(),
                code: evt.code,
            });
        }
    }

    counters
}

fn resolve_custom_for_family(name: &str, family_id: &str) -> Option<Counter> {
    let family = cpu_family::find_cpu_family(family_id)?;
    let event = family.events.get(name).or_else(|| {
        family
            .events
            .values()
            .find(|event| event.name.eq_ignore_ascii_case(name))
    })?;
    Some(Counter::Internal {
        name: event.name.clone(),
        desc: event.desc.clone(),
        code: event.code,
    })
}

/// Resolve a logical counter into the concrete event for a *specific* CPU
/// family, without assuming it is the host family. Used to open the same
/// logical counter on every cluster's PMU for faithful per-core counting.
///
/// Returns `None` when the counter is a named event that the given family does
/// not implement (e.g. an A720-only microarchitectural event has no meaning on
/// the A520 cluster), so callers simply skip it there instead of counting a
/// differently-numbered event.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub fn resolve_counter_for_family(
    counter: &Counter,
    family_id: &str,
    prefer_raw_counters: bool,
) -> Option<Counter> {
    let info = cpu_family::find_cpu_family(family_id)?;

    match counter {
        // Software counters are not PMU-specific.
        c if c.is_software() => Some(c.clone()),

        Counter::Custom(name) => resolve_custom_for_family(name, family_id),

        // Already a concrete raw event: assume the caller knows it is valid for
        // this family (it originates from this family's event table).
        Counter::Internal { .. } => Some(counter.clone()),

        // Generic hardware counters: remap to this family's architectural event
        // via the alias table when possible, otherwise keep the generic form.
        _ if prefer_raw_counters => {
            let alias_name = match counter {
                Counter::Cycles => "cycles",
                Counter::Instructions => "instructions",
                Counter::LLCMisses => "cache_misses",
                Counter::LLCReferences => "cache_references",
                Counter::BranchMisses => "branch_misses",
                Counter::BranchInstructions => "branches",
                Counter::StalledCyclesBackend => "stalled_cycles_backend",
                Counter::StalledCyclesFrontend => "stalled_cycles_frontend",
                _ => return Some(counter.clone()),
            };

            match info
                .aliases
                .get(alias_name)
                .and_then(|o| info.events.get(o))
            {
                Some(evt) => Some(Counter::Internal {
                    name: evt.name.clone(),
                    desc: evt.desc.clone(),
                    code: evt.code,
                }),
                None => Some(counter.clone()),
            }
        }

        _ => Some(counter.clone()),
    }
}

pub fn process_counter(
    counter: &Counter,
    prefer_raw_counters: bool,
) -> Result<Counter, crate::Error> {
    if let Counter::Custom(name) = counter {
        let cpu_family = cpu_family::get_host_cpu_family();
        cpu_family::find_cpu_family(cpu_family).ok_or_else(|| {
            crate::Error::UnsupportedCounter {
                counter: name.clone(),
                family: cpu_family.to_owned(),
            }
        })?;
        return resolve_custom_for_family(name, cpu_family).ok_or_else(|| {
            crate::Error::UnsupportedCounter {
                counter: name.clone(),
                family: cpu_family.to_owned(),
            }
        });
    } else if prefer_raw_counters {
        let cpu_family = cpu_family::get_host_cpu_family();
        let Some(info) = cpu_family::find_cpu_family(cpu_family) else {
            return Ok(counter.clone());
        };

        let alias_name = match counter {
            Counter::Cycles => "cycles",
            Counter::Instructions => "instructions",
            Counter::LLCMisses => "cache_misses",
            Counter::LLCReferences => "cache_references",
            Counter::BranchMisses => "branch_misses",
            Counter::BranchInstructions => "branches",
            Counter::StalledCyclesBackend => "stalled_cycles_backend",
            Counter::StalledCyclesFrontend => "stalled_cycles_frontend",
            _ => return Ok(counter.clone()),
        };

        let Some(alias) = info.aliases.get(alias_name) else {
            return Ok(counter.clone());
        };
        let Some(new_counter) = info.events.get(alias) else {
            return Ok(counter.clone());
        };

        return Ok(Counter::Internal {
            name: new_counter.name.clone(),
            desc: new_counter.desc.clone(),
            code: new_counter.code,
        });
    }

    Ok(counter.clone())
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn resolves_tiger_lake_custom_event_case_insensitively() {
        let counter = resolve_custom_for_family("l1d.replacement", pmu_data::INTEL_TIGERLAKE)
            .expect("Tiger Lake event must resolve");
        assert!(matches!(
            counter,
            Counter::Internal { ref name, code: 0x151, .. } if name == "L1D.REPLACEMENT"
        ));
    }
}
