use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsStr,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use pmu_data::{Alias, EventDesc, Metric, MetricExpression, PlatformDesc};
use serde_json::Value;

pub type ImportResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Converts Intel perfmon or Linux perf core event and metric JSON files.
///
/// `source` may be a direct Intel/perf-compatible JSON file or a Linux perf
/// family directory containing multiple JSON files.
pub fn import_intel(source: &Path, family_id: &str, name: &str) -> ImportResult<PlatformDesc> {
    let files = json_sources(source)?;

    let mut events = BTreeMap::<String, EventDesc>::new();
    let mut metrics = BTreeMap::<String, Metric>::new();
    for path in files {
        let document: Value = serde_json::from_reader(BufReader::new(File::open(&path)?))?;
        for value in intel_records(document, &path)? {
            if let Some(event) = convert_event(&value)? {
                events.insert(event.name.clone(), event);
            }
            if let Some(metric) = convert_metric(&value) {
                metrics.insert(metric.name.clone(), metric);
            }
        }
    }

    let aliases = intel_aliases(&events);
    Ok(PlatformDesc {
        family_id: family_id.to_owned(),
        name: name.to_owned(),
        vendor: "Intel".to_owned(),
        arch: "x86_64".to_owned(),
        max_counters: Some(8),
        leader_event: None,
        events: events.into_values().collect(),
        aliases: Some(aliases),
        metrics: metrics.into_values().collect(),
        scenarios: None,
    })
}

/// Backwards-compatible name for importing a Linux perf family directory.
pub fn import_intel_linux(
    source: &Path,
    family_id: &str,
    name: &str,
) -> ImportResult<PlatformDesc> {
    import_intel(source, family_id, name)
}

/// Converts an Arm Telemetry Solution PMU JSON file.
pub fn import_arm_telemetry(
    source: &Path,
    family_id: &str,
    name: &str,
) -> ImportResult<PlatformDesc> {
    let document: Value = serde_json::from_reader(BufReader::new(File::open(source)?))?;
    let event_values = document
        .get("events")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{} does not contain an events object", source.display()))?;

    let mut events = BTreeMap::<String, EventDesc>::new();
    for (mnemonic, value) in event_values {
        if !arm_event_is_pmu_accessible(value)? {
            continue;
        }
        let code = value
            .get("code")
            .ok_or_else(|| format!("Arm event {mnemonic} is missing its code"))?;
        let code = parse_json_number(code, "code")?;
        let desc = string_field(value, "description")
            .or_else(|| string_field(value, "title"))
            .unwrap_or("");
        events.insert(
            mnemonic.clone(),
            EventDesc {
                name: mnemonic.clone(),
                desc: desc.to_owned(),
                code,
            },
        );
    }

    let aliases = arm_aliases(&events);
    Ok(PlatformDesc {
        family_id: family_id.to_owned(),
        name: name.to_owned(),
        vendor: "ARM".to_owned(),
        arch: "aarch64".to_owned(),
        max_counters: None,
        leader_event: None,
        events: events.into_values().collect(),
        aliases: Some(aliases),
        metrics: Vec::new(),
        scenarios: None,
    })
}

fn json_sources(source: &Path) -> ImportResult<Vec<PathBuf>> {
    if source.is_file() {
        if source.extension() != Some(OsStr::new("json")) {
            return Err(format!("{} is not a JSON file", source.display()).into());
        }
        return Ok(vec![source.to_owned()]);
    }

    let mut files: Vec<PathBuf> = fs::read_dir(source)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("json")))
        .filter(|path| {
            !path
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .starts_with("uncore-")
        })
        .collect();
    files.sort();
    Ok(files)
}

fn intel_records(document: Value, path: &Path) -> ImportResult<Vec<Value>> {
    match document {
        Value::Array(records) => Ok(records),
        Value::Object(mut object) => object
            .remove("Events")
            .or_else(|| object.remove("events"))
            .and_then(|value| value.as_array().cloned())
            .ok_or_else(|| {
                format!(
                    "{} must be an event array or contain an Events array",
                    path.display()
                )
                .into()
            }),
        _ => Err(format!("{} must contain a JSON event array", path.display()).into()),
    }
}

fn arm_event_is_pmu_accessible(value: &Value) -> ImportResult<bool> {
    let Some(accesses) = value.get("accesses") else {
        return Ok(true);
    };
    let accesses = accesses
        .as_array()
        .ok_or("Arm event accesses must be an array")?;
    Ok(accesses.iter().any(|access| access.as_str() == Some("PMU")))
}

pub fn convert_event(value: &Value) -> ImportResult<Option<EventDesc>> {
    let Some(name) = string_field(value, "EventName") else {
        return Ok(None);
    };
    let Some(event_code) = string_field(value, "EventCode") else {
        return Ok(None);
    };
    if value.get("MSRIndex").is_some() || value.get("MSRValue").is_some() {
        return Ok(None);
    }

    let mut code = parse_number(event_code)?;
    code |= parse_optional(value, "UMask")? << 8;
    code |= parse_optional(value, "EdgeDetect")? << 18;
    code |= parse_optional(value, "AnyThread")? << 21;
    code |= parse_optional(value, "Invert")? << 23;
    code |= parse_optional(value, "CounterMask")? << 24;

    let desc = string_field(value, "PublicDescription")
        .or_else(|| string_field(value, "BriefDescription"))
        .unwrap_or("")
        .to_owned();
    Ok(Some(EventDesc {
        name: name.to_owned(),
        desc,
        code,
    }))
}

pub fn convert_metric(value: &Value) -> Option<Metric> {
    let name = string_field(value, "MetricName")?;
    let expression = string_field(value, "MetricExpr")?;
    let desc = string_field(value, "PublicDescription")
        .or_else(|| string_field(value, "BriefDescription"))
        .unwrap_or("");
    Some(Metric {
        name: name.to_owned(),
        desc: desc.to_owned(),
        expression: MetricExpression(expression.to_owned()),
        unit: string_field(value, "ScaleUnit").map(str::to_owned),
    })
}

fn intel_aliases(events: &BTreeMap<String, EventDesc>) -> Vec<Alias> {
    const ALIASES: &[(&str, &[&str])] = &[
        (
            "cycles",
            &["CPU_CLK_UNHALTED.THREAD_P", "CPU_CLK_UNHALTED.THREAD"],
        ),
        ("instructions", &["INST_RETIRED.ANY_P", "INST_RETIRED.ANY"]),
        ("branches", &["BR_INST_RETIRED.ALL_BRANCHES"]),
        ("branch_misses", &["BR_MISP_RETIRED.ALL_BRANCHES"]),
        ("cache_references", &["LONGEST_LAT_CACHE.REFERENCE"]),
        ("cache_misses", &["LONGEST_LAT_CACHE.MISS"]),
    ];

    ALIASES
        .iter()
        .filter_map(|(target, origins)| {
            origins
                .iter()
                .find(|origin| events.contains_key(**origin))
                .map(|origin| Alias {
                    target: (*target).to_owned(),
                    origin: (*origin).to_owned(),
                })
        })
        .collect()
}

fn arm_aliases(events: &BTreeMap<String, EventDesc>) -> Vec<Alias> {
    [
        ("cycles", "CPU_CYCLES"),
        ("instructions", "INST_RETIRED"),
        ("branches", "BR_RETIRED"),
        ("branch_misses", "BR_MIS_PRED_RETIRED"),
        ("cache_references", "LL_CACHE_RD"),
        ("cache_misses", "LL_CACHE_MISS_RD"),
        ("stalled_cycles_frontend", "STALL_FRONTEND"),
        ("stalled_cycles_backend", "STALL_BACKEND"),
    ]
    .into_iter()
    .filter(|(_, origin)| events.contains_key(*origin))
    .map(|(target, origin)| Alias {
        target: target.to_owned(),
        origin: origin.to_owned(),
    })
    .collect()
}

fn string_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn parse_optional(value: &Value, name: &str) -> ImportResult<u64> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::String(raw)) if raw.is_empty() => Ok(0),
        Some(Value::String(raw)) => parse_number(raw),
        Some(Value::Number(raw)) => raw
            .as_u64()
            .ok_or_else(|| format!("{name} is not an unsigned integer").into()),
        Some(other) => Err(format!("unsupported {name} value: {other}").into()),
    }
}

fn parse_json_number(value: &Value, name: &str) -> ImportResult<u64> {
    match value {
        Value::String(raw) => parse_number(raw),
        Value::Number(raw) => raw
            .as_u64()
            .ok_or_else(|| format!("{name} is not an unsigned integer").into()),
        other => Err(format!("unsupported {name} value: {other}").into()),
    }
}

fn parse_number(raw: &str) -> ImportResult<u64> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Ok(u64::from_str_radix(hex, 16)?)
    } else {
        Ok(raw.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_edge_invert_and_counter_mask_fields() {
        let value = serde_json::json!({
            "EventName": "TEST.EVENT",
            "EventCode": "0x48",
            "UMask": "0x2",
            "CounterMask": "1",
            "EdgeDetect": "1",
            "Invert": "1"
        });
        assert_eq!(convert_event(&value).unwrap().unwrap().code, 0x0184_0248);
    }

    #[test]
    fn imports_perf_metric_definition() {
        let value = serde_json::json!({
            "MetricName": "IPC",
            "MetricExpr": "instructions / cycles",
            "BriefDescription": "Instructions per cycle",
            "ScaleUnit": "1insn/cycle"
        });
        let metric = convert_metric(&value).unwrap();
        assert_eq!(metric.name, "IPC");
        assert_eq!(metric.expression.0, "instructions / cycles");
        assert_eq!(metric.unit.as_deref(), Some("1insn/cycle"));
    }

    #[test]
    fn ignores_fixed_only_and_extra_register_events() {
        let fixed = serde_json::json!({"EventName": "INST_RETIRED.ANY", "UMask": "0x1"});
        let offcore = serde_json::json!({
            "EventName": "OFFCORE_RESPONSE.TEST", "EventCode": "0xb7",
            "MSRIndex": "0x1a6", "MSRValue": "0x1"
        });
        assert!(convert_event(&fixed).unwrap().is_none());
        assert!(convert_event(&offcore).unwrap().is_none());
    }

    #[test]
    fn imports_direct_intel_perfmon_fixture() {
        let source = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/intel-perfmon.json"
        ));
        let platform = import_intel(source, "fixture", "Intel fixture").unwrap();
        assert_eq!(platform.events.len(), 2);
        assert_eq!(platform.events[0].name, "CPU_CLK_UNHALTED.THREAD_P");
        assert_eq!(platform.events[0].code, 0x3c);
        assert_eq!(platform.events[1].code, 0x1c0);
        let aliases = platform.aliases.unwrap();
        assert_eq!(aliases[0].target, "cycles");
        assert_eq!(aliases[1].target, "instructions");
    }

    #[test]
    fn imports_arm_telemetry_fixture() {
        let source = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/arm-telemetry.json"
        ));
        let platform = import_arm_telemetry(source, "fixture", "Arm fixture").unwrap();
        assert_eq!(platform.vendor, "ARM");
        assert_eq!(platform.arch, "aarch64");
        assert_eq!(platform.events.len(), 3);
        assert_eq!(platform.events[0].name, "BR_RETIRED");
        assert_eq!(platform.events[0].code, 0x21);
        assert_eq!(platform.events[1].desc, "Counts processor cycles.");
        assert_eq!(platform.aliases.unwrap().len(), 3);
    }
}
