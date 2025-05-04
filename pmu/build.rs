use std::{
    error::Error,
    fs::{self, File},
};

use glob::glob;
use pmu_data::PlatformDesc;
use quote::quote;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=events/");

    let mut families = vec![];
    for entry in glob("events/**/*.json")? {
        let path = entry?;
        println!("cargo:rerun-if-changed={}", path.display());

        let file = File::open(path)?;
        let data: PlatformDesc = serde_json::from_reader(file)?;

        let arch = std::env::var("CARGO_CFG_TARGET_ARCH")?;

        let native_events = std::env::var_os("CARGO_FEATURE_EVENTS_NATIVE").is_some();
        let arch_feature = match data.arch.as_str() {
            "x86_64" => "CARGO_FEATURE_EVENTS_X86_64",
            "aarch64" => "CARGO_FEATURE_EVENTS_AARCH64",
            "riscv64" | "riscv64gc" => "CARGO_FEATURE_EVENTS_RISCV64",
            _ => continue,
        };
        let selected =
            (native_events && arch == data.arch) || std::env::var_os(arch_feature).is_some();

        if !selected {
            continue;
        }

        let mut events = vec![];

        for evt in data.events {
            let name = &evt.name;
            let desc = &evt.desc;
            let code = evt.code;
            events.push(quote! {
                events.insert(#name.to_string(), EventDesc {
                    name: #name.to_string(),
                    desc: #desc.to_string(),
                    code: #code
                });
            });
        }

        let mut aliases = vec![];

        for alias in &data.aliases.unwrap_or_default() {
            let target = alias.target.clone();
            let origin = alias.origin.clone();
            aliases.push(quote! {
                aliases.insert(#target.to_string(), #origin.to_string());
            });
        }

        let mut scenarios = vec![];
        for scenario in &data.scenarios.unwrap_or_default() {
            let name = scenario.name.clone();
            let events = scenario
                .events
                .iter()
                .map(|event| quote! { #event.to_string() });
            let constants = scenario.constants.iter().map(|constant| {
                let name = constant.name.clone();
                let value = constant.value;
                quote! {
                    pmu_data::TmaConstant { name: #name.to_string(), value: #value }
                }
            });
            let scenario_metrics = scenario.metrics.iter().map(|metric| {
                let name = metric.name.clone();
                let desc = metric.desc.clone();
                let formula = metric.formula.clone();
                quote! {
                    pmu_data::TmaMetric {
                        name: #name.to_string(),
                        desc: #desc.to_string(),
                        formula: #formula.to_string(),
                    }
                }
            });
            scenarios.push(quote! {
                scenarios.insert(#name.to_string(), pmu_data::TmaScenario {
                    name: #name.to_string(),
                    events: vec![#(#events),*],
                    constants: vec![#(#constants),*],
                    metrics: vec![#(#scenario_metrics),*],
                });
            });
        }

        let name = &data.name;
        let vendor = &data.vendor;
        let family_id = &data.family_id;
        let max_counters = data
            .max_counters
            .map(|num| quote! {Some(#num)})
            .unwrap_or(quote! { None });
        let leader_event = data
            .leader_event
            .map(|l| quote! {Some(#l.to_string())})
            .unwrap_or(quote! { None });
        let metrics = data.metrics.into_iter().map(|metric| {
            let metric_name = metric.name;
            let desc = metric.desc;
            let expression = metric.expression.0;
            let unit = metric
                .unit
                .map(|unit| quote! { Some(#unit.to_string()) })
                .unwrap_or(quote! { None });
            quote! {
                Metric {
                    name: #metric_name.to_string(),
                    desc: #desc.to_string(),
                    expression: pmu_data::MetricExpression(#expression.to_string()),
                    unit: #unit,
                }
            }
        });

        families.push(quote! {
            let mut events = HashMap::new();
            #(#events)*

            #[allow(unused_mut)]
            let mut aliases = HashMap::new();

            #(#aliases)*

            #[allow(unused_mut)]
            let mut scenarios = HashMap::new();

            #(#scenarios)*

            let family = CPUFamily {
                name: #name.to_string(),
                vendor: #vendor.to_string(),
                id: #family_id.to_string(),
                leader_event: #leader_event,
                max_counters: #max_counters,
                events,
                aliases,
                metrics: vec![#(#metrics),*],
                scenarios,
            };

            families.insert(#family_id.to_string(), family);
        });
    }

    let all_events = quote! {
        lazy_static! {
            static ref CPU_FAMILIES: HashMap<String, CPUFamily> = create_known_counters_map();
        }

        fn create_known_counters_map() -> HashMap<String, CPUFamily> {
            #[allow(unused_mut)]
            let mut families = HashMap::new();

            #(#families)*

            families
        }
    };

    let file = syn::parse2(all_events)?;
    let formatted = prettyplease::unparse(&file);

    fs::write(
        format!("{}/events.rs", std::env::var("OUT_DIR")?),
        formatted,
    )?;

    Ok(())
}
