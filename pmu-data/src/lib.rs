pub mod arith_parser;

use serde::{Deserialize, Serialize, de};

// Well-known CPU families
pub const AMDZEN1: &str = "zen1";
pub const AMDZEN2: &str = "zen2";
pub const AMDZEN3: &str = "zen3";
pub const AMDZEN4: &str = "zen4";

pub const INTEL_HASWELL: &str = "haswell";
pub const INTEL_BROADWELL: &str = "broadwell";
pub const INTEL_SKYLAKE: &str = "skylake";
pub const INTEL_KABYLAKE: &str = "kabylake";
pub const INTEL_COMETLAKE: &str = "cometlake";
pub const INTEL_ICELAKE: &str = "icelake";
// a.k.a. Ice Lake Server
pub const INTEL_ICX: &str = "icx";
pub const INTEL_TIGERLAKE: &str = "tigerlake";
pub const INTEL_ROCKETLAKE: &str = "rocketlake";
pub const INTEL_ALDERLAKE: &str = "alderlake";
pub const INTEL_RAPTORLAKE: &str = "raptorlake";

pub const SIFIVE_U7: &str = "sifive_u7";

pub const SPACEMIT_X60: &str = "spacemit_x60";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlatformDesc {
    pub family_id: String,
    pub name: String,
    pub vendor: String,
    pub arch: String,
    pub max_counters: Option<usize>,
    pub leader_event: Option<String>,
    pub scenarios: Option<Vec<Scenario>>,
    pub events: Vec<EventDesc>,
    pub aliases: Option<Vec<Alias>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventDesc {
    pub name: String,
    pub desc: String,
    #[serde(serialize_with = "serialize_hex", deserialize_with = "deserialize_hex")]
    pub code: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alias {
    pub target: String,
    pub origin: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub events: Vec<String>,
    pub constants: Vec<Constant>,
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub ui: Option<ScenarioUi>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub desc: String,
    pub formula: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Constant {
    pub name: String,
    pub value: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ScenarioUi {
    #[serde(default)]
    pub tabs: Vec<TabSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabSpec {
    Summary,
    Flamegraph,
    Loops,
    MetricsTable(MetricsTableSpec),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsTableSpec {
    pub view: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_true")]
    pub include_default_columns: bool,
    #[serde(default)]
    pub columns: Vec<MetricColumnSpec>,
    #[serde(default)]
    pub order_by: Option<OrderSpec>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub sticky_columns: Option<usize>,
    #[serde(default)]
    pub function_column: Option<String>,
    #[serde(default)]
    pub enable_assembly: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderSpec {
    pub column: String,
    #[serde(default)]
    pub direction: SortDirection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

impl Default for SortDirection {
    fn default() -> Self {
        SortDirection::Desc
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricColumnSpec {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub format: ValueFormat,
    #[serde(default)]
    pub width: Option<u16>,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueFormat {
    Auto,
    Text,
    Integer,
    Float,
    Float1,
    Float2,
    Float3,
    Percent,
    Percent1,
    Percent2,
    Percent3,
}

impl Default for ValueFormat {
    fn default() -> Self {
        ValueFormat::Auto
    }
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_ui_parses_from_json() {
        let json = r#"
        {
            "name": "tma",
            "events": [],
            "constants": [],
            "metrics": [],
            "ui": {
                "tabs": [
                    { "kind": "summary" },
                    {
                        "kind": "metrics_table",
                        "view": "hotspots",
                        "columns": [
                            { "key": "func_name", "label": "Function", "format": "text", "sticky": true }
                        ]
                    }
                ]
            }
        }
        "#;

        let scenario: Scenario = serde_json::from_str(json).expect("failed to parse scenario json");
        assert!(scenario.ui.is_some());
        let ui = scenario.ui.unwrap();
        assert_eq!(ui.tabs.len(), 2);
        match &ui.tabs[1] {
            TabSpec::MetricsTable(spec) => {
                assert_eq!(spec.view, "hotspots");
                assert!(spec.include_default_columns);
                assert_eq!(spec.columns.len(), 1);
                assert_eq!(spec.columns[0].key, "func_name");
            }
            _ => panic!("expected metrics_table tab"),
        }
    }
}

fn serialize_hex<S>(v: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let string = format!("0x{:X}", v);
    serializer.serialize_str(&string)
}

fn deserialize_hex<'a, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'a>,
{
    struct Visitor;

    impl de::Visitor<'_> for Visitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string containing a hexadecimal number starting with '0x'")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if !v.starts_with("0x") {
                return Err(E::custom("does not start with '0x'"));
            }

            let hex_only = &v[2..];
            match u64::from_str_radix(hex_only, 16) {
                Ok(value) => Ok(value),
                Err(err) => Err(E::custom(err)),
            }
        }
    }

    deserializer.deserialize_str(Visitor)
}
