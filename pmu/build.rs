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

        if arch != data.arch {
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
            let events = scenario.events.iter().map(|evt| quote! { #evt.to_string() });
            let constants = scenario.constants.iter().map(|cst| {
                let name = cst.name.clone();
                let value = cst.value;

                quote! {
                    pmu_data::Constant {
                        name: #name.to_string(),
                        value: #value
                    }
                }
            });

            let metrics = scenario.metrics.iter().map(|metric| {
                let name = metric.name.clone();
                let desc = metric.desc.clone();
                let formula = metric.formula.clone();

                quote! {
                    pmu_data::Metric {
                        name: #name.to_string(),
                        desc: #desc.to_string(),
                        formula: #formula.to_string(),
                    }
                }
            });

            scenarios.push(quote!{
                scenarios.insert(#name.to_string(), pmu_data::Scenario {
                    name: #name.to_string(),
                    events: vec![#(#events),*],
                    constants: vec![#(#constants),*],
                    metrics: vec![#(#metrics),*],
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
