use serde::{de, Deserialize, Serialize};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlatformDesc {
    pub family_id: String,
    pub name: String,
    pub vendor: String,
    pub arch: String,
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
    target: String,
    origin: String,
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
