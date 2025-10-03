use std::{
    error::Error,
    fs::{self, File},
};

use glob::glob;
use pmu_data::{
    MetricColumnSpec, MetricsTableSpec, OrderSpec, PlatformDesc, ScenarioUi, SortDirection,
    TabSpec, ValueFormat,
};
use proc_macro2::TokenStream;
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
            let events = scenario
                .events
                .iter()
                .map(|evt| quote! { #evt.to_string() });
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

            let ui = scenario
                .ui
                .as_ref()
                .map(|ui| scenario_ui_tokens(ui))
                .unwrap_or_else(|| quote! { None });

            scenarios.push(quote! {
                scenarios.insert(#name.to_string(), pmu_data::Scenario {
                    name: #name.to_string(),
                    events: vec![#(#events),*],
                    constants: vec![#(#constants),*],
                    metrics: vec![#(#metrics),*],
                    ui: #ui,
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

fn scenario_ui_tokens(ui: &ScenarioUi) -> TokenStream {
    let tabs = ui.tabs.iter().map(tab_tokens);

    quote! {
        Some(pmu_data::ScenarioUi {
            tabs: vec![#(#tabs),*],
        })
    }
}

fn tab_tokens(tab: &TabSpec) -> TokenStream {
    match tab {
        TabSpec::Summary => quote! { pmu_data::TabSpec::Summary },
        TabSpec::Flamegraph => quote! { pmu_data::TabSpec::Flamegraph },
        TabSpec::Loops => quote! { pmu_data::TabSpec::Loops },
        TabSpec::MetricsTable(spec) => {
            let metrics_spec = metrics_table_tokens(spec);
            quote! { pmu_data::TabSpec::MetricsTable(#metrics_spec) }
        }
    }
}

fn metrics_table_tokens(spec: &MetricsTableSpec) -> TokenStream {
    let view = &spec.view;
    let title = option_string_tokens(spec.title.as_ref());
    let columns = spec.columns.iter().map(metric_column_tokens);
    let order_by = option_order_tokens(spec.order_by.as_ref());
    let limit = option_usize_tokens(spec.limit);
    let sticky_columns = option_usize_tokens(spec.sticky_columns);
    let function_column = option_string_tokens(spec.function_column.as_ref());
    let enable_assembly = spec.enable_assembly;
    let include_defaults = spec.include_default_columns;

    quote! {
        pmu_data::MetricsTableSpec {
            view: #view.to_string(),
            title: #title,
            include_default_columns: #include_defaults,
            columns: vec![#(#columns),*],
            order_by: #order_by,
            limit: #limit,
            sticky_columns: #sticky_columns,
            function_column: #function_column,
            enable_assembly: #enable_assembly,
        }
    }
}

fn metric_column_tokens(column: &MetricColumnSpec) -> TokenStream {
    let key = &column.key;
    let label = option_string_tokens(column.label.as_ref());
    let format = value_format_tokens(&column.format);
    let width = option_u16_tokens(column.width);
    let sticky = column.sticky;
    let optional = column.optional;

    quote! {
        pmu_data::MetricColumnSpec {
            key: #key.to_string(),
            label: #label,
            format: #format,
            width: #width,
            sticky: #sticky,
            optional: #optional,
        }
    }
}

fn option_string_tokens(value: Option<&String>) -> TokenStream {
    value
        .map(|v| quote! { Some(#v.to_string()) })
        .unwrap_or_else(|| quote! { None })
}

fn option_usize_tokens(value: Option<usize>) -> TokenStream {
    value
        .map(|v| quote! { Some(#v) })
        .unwrap_or_else(|| quote! { None })
}

fn option_u16_tokens(value: Option<u16>) -> TokenStream {
    value
        .map(|v| quote! { Some(#v) })
        .unwrap_or_else(|| quote! { None })
}

fn option_order_tokens(value: Option<&OrderSpec>) -> TokenStream {
    match value {
        Some(order) => {
            let column = &order.column;
            let direction = sort_direction_tokens(&order.direction);
            quote! {
                Some(pmu_data::OrderSpec {
                    column: #column.to_string(),
                    direction: #direction,
                })
            }
        }
        None => quote! { None },
    }
}

fn sort_direction_tokens(direction: &SortDirection) -> TokenStream {
    match direction {
        SortDirection::Asc => quote! { pmu_data::SortDirection::Asc },
        SortDirection::Desc => quote! { pmu_data::SortDirection::Desc },
    }
}

fn value_format_tokens(value: &ValueFormat) -> TokenStream {
    match value {
        ValueFormat::Auto => quote! { pmu_data::ValueFormat::Auto },
        ValueFormat::Text => quote! { pmu_data::ValueFormat::Text },
        ValueFormat::Integer => quote! { pmu_data::ValueFormat::Integer },
        ValueFormat::Float => quote! { pmu_data::ValueFormat::Float },
        ValueFormat::Float1 => quote! { pmu_data::ValueFormat::Float1 },
        ValueFormat::Float2 => quote! { pmu_data::ValueFormat::Float2 },
        ValueFormat::Float3 => quote! { pmu_data::ValueFormat::Float3 },
        ValueFormat::Percent => quote! { pmu_data::ValueFormat::Percent },
        ValueFormat::Percent1 => quote! { pmu_data::ValueFormat::Percent1 },
        ValueFormat::Percent2 => quote! { pmu_data::ValueFormat::Percent2 },
        ValueFormat::Percent3 => quote! { pmu_data::ValueFormat::Percent3 },
    }
}
