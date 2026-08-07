use std::path::PathBuf;

use anyhow::{Context, Result};
use mperf_data::{RooflineCalibration, RooflineMethodInfo};
use sqlite::{Connection, Value};

use crate::source::SourceLocation;

const LABEL_ASSET_PREFIX: &str = "roofline-label:";

pub(crate) fn roofline_label_asset(text: &str) -> String {
    format!("{LABEL_ASSET_PREFIX}{text}")
}

pub(crate) fn roofline_label_svg(path: &str) -> Option<Vec<u8>> {
    let text = path.strip_prefix(LABEL_ASSET_PREFIX)?;
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Some(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 176 18"><text x="88" y="13" text-anchor="middle" font-family="sans-serif" font-size="11" font-weight="500" fill="black">{escaped}</text></svg>"#,
        )
        .into_bytes(),
    )
}

#[derive(Debug)]
pub struct RooflineData {
    pub loops: Vec<RooflineLoop>,
    pub calibration: Option<RooflineCalibration>,
    pub method: Option<RooflineMethodInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RooflineLoop {
    pub function_name: String,
    pub file_name: String,
    pub line: usize,
    pub scalar_int_ops: Option<f64>,
    pub scalar_int_ai: Option<f64>,
    pub scalar_float_ops: Option<f64>,
    pub scalar_float_ai: Option<f64>,
    pub scalar_double_ops: Option<f64>,
    pub scalar_double_ai: Option<f64>,
    pub vector_int_ops: Option<f64>,
    pub vector_int_ai: Option<f64>,
    pub vector_float_ops: Option<f64>,
    pub vector_float_ai: Option<f64>,
    pub vector_double_ops: Option<f64>,
    pub vector_double_ai: Option<f64>,
    pub timing_samples: Option<u64>,
    pub timing_relative_error: Option<f64>,
    pub timing_quality: Option<String>,
    pub module_offset: Option<String>,
    pub trip_count: Option<u64>,
}

impl RooflineData {
    pub fn load(
        connection: &Connection,
        calibration: Option<RooflineCalibration>,
        method: Option<RooflineMethodInfo>,
    ) -> RooflineData {
        match load_loops(connection) {
            Ok(loops) => Self {
                loops,
                calibration,
                method,
                error: None,
            },
            Err(error) => Self {
                loops: Vec::new(),
                calibration,
                method,
                error: Some(format!("{error:#}")),
            },
        }
    }

    /// Calibrated bandwidth roofs that match this recording's intensity axis.
    /// Architectural traffic uses the complete cache-aware hierarchy, while
    /// DRAM traffic uses only the DRAM-sized streaming roof.
    pub fn bandwidth_roofs(&self) -> Vec<(&str, f64)> {
        let Some(calibration) = self.calibration.as_ref() else {
            return Vec::new();
        };
        let Some(method) = self.method.as_ref() else {
            return Vec::new();
        };

        let mut roofs = match method.traffic.as_str() {
            "architectural" => calibration
                .memory_levels
                .iter()
                .map(|level| (level.level.as_str(), level.gbytes_per_second))
                .collect::<Vec<_>>(),
            "dram" | "dram-model" => vec![("DRAM", calibration.memory_gbytes_per_second)],
            _ => Vec::new(),
        };
        roofs.retain(|(_, bandwidth)| bandwidth.is_finite() && *bandwidth > 0.0);
        roofs.sort_by(|left, right| right.1.total_cmp(&left.1));
        roofs
    }

    pub fn has_compatible_memory_roof(&self) -> bool {
        !self.bandwidth_roofs().is_empty()
    }

    pub fn uses_architectural_traffic(&self) -> bool {
        self.method
            .as_ref()
            .is_some_and(|method| method.traffic == "architectural")
    }

    pub fn uses_modeled_traffic(&self) -> bool {
        self.method
            .as_ref()
            .is_some_and(|method| method.traffic == "dram-model")
    }

    pub fn efficiency(&self, loop_data: &RooflineLoop) -> Option<f64> {
        let calibration = self.calibration.as_ref()?;
        let bandwidth = self
            .bandwidth_roofs()
            .into_iter()
            .map(|(_, bandwidth)| bandwidth)
            .max_by(f64::total_cmp)?;
        let observed = loop_data.fp64_gflops()?;
        let intensity = loop_data.fp64_arithmetic_intensity()?;
        let roof = finite_positive(calibration.fp64_gflops.min(bandwidth * intensity))?;
        (observed / roof).is_finite().then_some(observed / roof)
    }
}

impl RooflineLoop {
    pub fn fp64_gflops(&self) -> Option<f64> {
        finite_positive(
            sum_optional(self.scalar_double_ops, self.vector_double_ops)? / 1_000_000_000.0,
        )
    }

    pub fn fp64_arithmetic_intensity(&self) -> Option<f64> {
        finite_positive(sum_optional(self.scalar_double_ai, self.vector_double_ai)?)
    }

    pub fn source(&self) -> Option<SourceLocation> {
        (!self.file_name.trim().is_empty() && self.line > 0).then(|| SourceLocation {
            path: PathBuf::from(&self.file_name),
            line: self.line,
        })
    }
}

fn load_loops(connection: &Connection) -> Result<Vec<RooflineLoop>> {
    let confidence_columns = if connection
        .prepare("SELECT timing_quality FROM roofline LIMIT 0")
        .is_ok()
    {
        "timing_samples, timing_relative_error, timing_quality, module_offset, trip_count"
    } else {
        "NULL AS timing_samples, NULL AS timing_relative_error, NULL AS timing_quality, NULL AS module_offset, NULL AS trip_count"
    };
    let query = format!(
        "
        SELECT
            function_name,
            file_name,
            line,
            scalar_int_ops,
            scalar_int_ai,
            scalar_float_ops,
            scalar_float_ai,
            scalar_double_ops,
            scalar_double_ai,
            vector_int_ops,
            vector_int_ai,
            vector_float_ops,
            vector_float_ai,
            vector_double_ops,
            vector_double_ai,
            {confidence_columns}
        FROM roofline
        ORDER BY
            COALESCE(scalar_double_ops, 0) + COALESCE(vector_double_ops, 0) DESC,
            function_name ASC,
            file_name ASC,
            line ASC;
    "
    );
    connection
        .prepare(query)
        .context("Roofline data is unavailable: failed to query SQLite view `roofline`")?
        .into_iter()
        .map(|row| {
            let row = row.context("failed to read a row from SQLite view `roofline`")?;
            Ok(RooflineLoop {
                function_name: string_value(&row["function_name"])
                    .unwrap_or_else(|| "[unknown loop]".to_string()),
                file_name: string_value(&row["file_name"]).unwrap_or_default(),
                line: integer_value(&row["line"]).unwrap_or_default().max(0) as usize,
                scalar_int_ops: finite_value(&row["scalar_int_ops"]),
                scalar_int_ai: finite_value(&row["scalar_int_ai"]),
                scalar_float_ops: finite_value(&row["scalar_float_ops"]),
                scalar_float_ai: finite_value(&row["scalar_float_ai"]),
                scalar_double_ops: finite_value(&row["scalar_double_ops"]),
                scalar_double_ai: finite_value(&row["scalar_double_ai"]),
                vector_int_ops: finite_value(&row["vector_int_ops"]),
                vector_int_ai: finite_value(&row["vector_int_ai"]),
                vector_float_ops: finite_value(&row["vector_float_ops"]),
                vector_float_ai: finite_value(&row["vector_float_ai"]),
                vector_double_ops: finite_value(&row["vector_double_ops"]),
                vector_double_ai: finite_value(&row["vector_double_ai"]),
                timing_samples: integer_value(&row["timing_samples"])
                    .and_then(|value| u64::try_from(value).ok()),
                timing_relative_error: finite_value(&row["timing_relative_error"]),
                timing_quality: string_value(&row["timing_quality"]),
                module_offset: string_value(&row["module_offset"]),
                trip_count: integer_value(&row["trip_count"])
                    .and_then(|value| u64::try_from(value).ok()),
            })
        })
        .collect()
}

fn string_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn integer_value(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Float(value) => Some(*value as i64),
        _ => None,
    }
}

fn finite_value(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Integer(value) => *value as f64,
        Value::Float(value) => *value,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn finite_positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn sum_optional(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mperf_data::MemoryLevelCalibration;

    fn calibration() -> RooflineCalibration {
        RooflineCalibration {
            threads: 4,
            cpu_affinity: Some("0-3".to_string()),
            samples: 5,
            compute_kernel: "test-fma".to_string(),
            fp64_gflops: 200.0,
            fp64_gflops_samples: vec![200.0],
            memory_gbytes_per_second: 50.0,
            memory_gbytes_per_second_samples: vec![50.0],
            ridge_point_flops_per_byte: 4.0,
            memory_working_set_bytes: 1024,
            memory_levels: vec![
                MemoryLevelCalibration {
                    level: "L1".to_string(),
                    gbytes_per_second: 400.0,
                    gbytes_per_second_samples: vec![400.0],
                    working_set_bytes: 64,
                },
                MemoryLevelCalibration {
                    level: "DRAM".to_string(),
                    gbytes_per_second: 50.0,
                    gbytes_per_second_samples: vec![50.0],
                    working_set_bytes: 1024,
                },
            ],
        }
    }

    fn method(traffic: &str) -> RooflineMethodInfo {
        RooflineMethodInfo {
            selection: "auto".to_string(),
            accounting: "qemu".to_string(),
            performance: "native".to_string(),
            traffic: traffic.to_string(),
            quality: "test".to_string(),
            reason: "test".to_string(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn loads_every_roofline_row_and_combines_fp64_scalar_and_vector_values() {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE roofline (
                    function_name TEXT,
                    file_name TEXT,
                    line INTEGER,
                    scalar_int_ops REAL,
                    scalar_int_ai REAL,
                    scalar_float_ops REAL,
                    scalar_float_ai REAL,
                    scalar_double_ops REAL,
                    scalar_double_ai REAL,
                    vector_int_ops REAL,
                    vector_int_ai REAL,
                    vector_float_ops REAL,
                    vector_float_ai REAL,
                    vector_double_ops REAL,
                    vector_double_ai REAL
                );
                INSERT INTO roofline VALUES
                    ('mixed', '/src/kernel.c', 42,
                     0, 0, 0, 0, 2000000000, 2,
                     0, 0, 0, 0, 6000000000, 6),
                    ('integer-only', '/src/kernel.c', 9,
                     100, 1, 0, 0, 0, 0,
                     200, 2, 0, 0, 0, 0);",
            )
            .unwrap();

        let data = RooflineData::load(&connection, Some(calibration()), None);

        assert_eq!(data.error, None);
        assert_eq!(data.loops.len(), 2);
        assert_eq!(data.loops[0].function_name, "mixed");
        assert_eq!(data.loops[0].fp64_gflops(), Some(8.0));
        assert_eq!(data.loops[0].fp64_arithmetic_intensity(), Some(8.0));
        assert_eq!(
            data.loops[0].source(),
            Some(SourceLocation {
                path: PathBuf::from("/src/kernel.c"),
                line: 42,
            })
        );
        assert_eq!(data.loops[1].function_name, "integer-only");
        assert_eq!(data.loops[1].fp64_gflops(), None);
    }

    #[test]
    fn computes_memory_and_compute_bound_efficiency_against_selected_roofs() {
        let mut loop_data = RooflineLoop {
            function_name: "loop".to_string(),
            file_name: String::new(),
            line: 0,
            scalar_int_ops: None,
            scalar_int_ai: None,
            scalar_float_ops: None,
            scalar_float_ai: None,
            scalar_double_ops: Some(25_000_000_000.0),
            scalar_double_ai: Some(1.0),
            vector_int_ops: None,
            vector_int_ai: None,
            vector_float_ops: None,
            vector_float_ai: None,
            vector_double_ops: None,
            vector_double_ai: None,
            timing_samples: None,
            timing_relative_error: None,
            timing_quality: None,
            module_offset: None,
            trip_count: None,
        };
        let architectural = RooflineData {
            loops: Vec::new(),
            calibration: Some(calibration()),
            method: Some(method("architectural")),
            error: None,
        };
        let dram = RooflineData {
            loops: Vec::new(),
            calibration: Some(calibration()),
            method: Some(method("dram")),
            error: None,
        };

        assert_eq!(architectural.efficiency(&loop_data), Some(0.125));
        assert_eq!(dram.efficiency(&loop_data), Some(0.5));

        loop_data.scalar_double_ops = Some(100_000_000_000.0);
        loop_data.scalar_double_ai = Some(8.0);
        assert_eq!(architectural.efficiency(&loop_data), Some(0.5));
        assert_eq!(dram.efficiency(&loop_data), Some(0.5));
    }

    #[test]
    fn reports_a_missing_roofline_view_without_hiding_the_tab() {
        let connection = sqlite::open(":memory:").unwrap();
        let data = RooflineData::load(&connection, Some(calibration()), None);

        assert!(data.loops.is_empty());
        assert!(
            data.error
                .as_deref()
                .is_some_and(|error| error.contains("roofline"))
        );
        assert!(data.calibration.is_some());
    }

    #[test]
    fn selects_cache_hierarchy_for_carm_and_dram_roof_for_dram_intensity() {
        let connection = sqlite::open(":memory:").unwrap();

        let architectural = RooflineData::load(
            &connection,
            Some(calibration()),
            Some(method("architectural")),
        );
        assert!(architectural.has_compatible_memory_roof());
        assert!(architectural.uses_architectural_traffic());
        assert_eq!(
            architectural.bandwidth_roofs(),
            vec![("L1", 400.0), ("DRAM", 50.0)]
        );

        let dram = RooflineData::load(&connection, Some(calibration()), Some(method("dram")));
        assert!(dram.has_compatible_memory_roof());
        assert_eq!(dram.bandwidth_roofs(), vec![("DRAM", 50.0)]);

        let modeled =
            RooflineData::load(&connection, Some(calibration()), Some(method("dram-model")));
        assert!(modeled.has_compatible_memory_roof());
        assert!(modeled.uses_modeled_traffic());
        assert_eq!(modeled.bandwidth_roofs(), vec![("DRAM", 50.0)]);

        let unknown = RooflineData::load(&connection, Some(calibration()), Some(method("unknown")));
        assert!(!unknown.has_compatible_memory_roof());
        assert!(unknown.bandwidth_roofs().is_empty());
    }

    #[test]
    fn roofline_label_asset_escapes_recorded_level_names() {
        let path = roofline_label_asset("L<&> · 1.00 GB/s");
        let svg = String::from_utf8(roofline_label_svg(&path).unwrap()).unwrap();

        assert!(svg.contains("L&lt;&amp;&gt; · 1.00 GB/s"));
        assert!(!svg.contains("L<&>"));
    }
}
