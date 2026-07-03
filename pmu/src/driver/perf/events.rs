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

/// Resolve a logical counter into the concrete event for a *specific* CPU
/// family, without assuming it is the host family. Used to open the same
/// logical counter on every cluster's PMU for faithful per-core counting.
///
/// Returns `None` when the counter is a named event that the given family does
/// not implement (e.g. an A720-only microarchitectural event has no meaning on
/// the A520 cluster), so callers simply skip it there instead of counting a
/// differently-numbered event.
pub fn resolve_counter_for_family(
    counter: &Counter,
    family_id: &str,
    prefer_raw_counters: bool,
) -> Option<Counter> {
    let info = cpu_family::find_cpu_family(family_id)?;

    match counter {
        // Software counters are not PMU-specific.
        c if c.is_software() => Some(c.clone()),

        Counter::Custom(name) => {
            let evt = info.events.get(name)?;
            Some(Counter::Internal {
                name: evt.name.clone(),
                desc: evt.desc.clone(),
                code: evt.code,
            })
        }

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

pub fn process_counter(counter: &Counter, prefer_raw_counters: bool) -> Counter {
    if let Counter::Custom(name) = counter {
        let cpu_family = cpu_family::get_host_cpu_family();
        let info = cpu_family::find_cpu_family(cpu_family);
        if info.is_none() {
            panic!("Unsupported CPU family '{}'", cpu_family);
        }

        let info = info.unwrap();

        let counter = info.events.get(name);

        if counter.is_none() {
            panic!(
                "Unsupported counter '{}' for CPU family '{}'",
                name, cpu_family
            );
        }

        let counter = counter.unwrap();

        return Counter::Internal {
            name: counter.name.clone(),
            desc: counter.desc.clone(),
            code: counter.code,
        };
    } else if prefer_raw_counters {
        let cpu_family = cpu_family::get_host_cpu_family();
        let info = cpu_family::find_cpu_family(cpu_family);
        if info.is_none() {
            return counter.clone();
        }

        let info = info.unwrap();

        let alias_name = match counter {
            Counter::Cycles => "cycles",
            Counter::Instructions => "instructions",
            Counter::LLCMisses => "cache_misses",
            Counter::LLCReferences => "cache_references",
            Counter::BranchMisses => "branch_misses",
            Counter::BranchInstructions => "branches",
            Counter::StalledCyclesBackend => "stalled_cycles_backend",
            Counter::StalledCyclesFrontend => "stalled_cycles_frontend",
            _ => return counter.clone(),
        };

        let alias = info.aliases.get(alias_name);

        if alias.is_none() {
            return counter.clone();
        }

        let new_counter = info.events.get(alias.unwrap());

        if new_counter.is_none() {
            return counter.clone();
        }

        let new_counter = new_counter.unwrap();

        return Counter::Internal {
            name: new_counter.name.clone(),
            desc: new_counter.desc.clone(),
            code: new_counter.code,
        };
    }

    counter.clone()
}
