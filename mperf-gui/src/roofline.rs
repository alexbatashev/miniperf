use std::path::PathBuf;

use anyhow::{Context, Result};
use mperf_data::RooflineCalibration;
use sqlite::{Connection, Value};

use crate::source::SourceLocation;

#[derive(Debug)]
pub struct RooflineData {
    pub loops: Vec<RooflineLoop>,
    pub calibration: Option<RooflineCalibration>,
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
}

impl RooflineData {
    pub fn load(connection: &Connection, calibration: Option<RooflineCalibration>) -> RooflineData {
        match load_loops(connection) {
            Ok(loops) => Self {
                loops,
                calibration,
                error: None,
            },
            Err(error) => Self {
                loops: Vec::new(),
                calibration,
                error: Some(format!("{error:#}")),
            },
        }
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

    pub fn relevant_roof_gflops(&self, calibration: &RooflineCalibration) -> Option<f64> {
        let intensity = self.fp64_arithmetic_intensity()?;
        finite_positive(
            calibration
                .fp64_gflops
                .min(calibration.memory_gbytes_per_second * intensity),
        )
    }

    pub fn efficiency(&self, calibration: &RooflineCalibration) -> Option<f64> {
        let observed = self.fp64_gflops()?;
        let roof = self.relevant_roof_gflops(calibration)?;
        (roof > 0.0)
            .then_some(observed / roof)
            .filter(|value| value.is_finite())
    }
}

fn load_loops(connection: &Connection) -> Result<Vec<RooflineLoop>> {
    let query = "
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
            vector_double_ai
        FROM roofline
        ORDER BY
            COALESCE(scalar_double_ops, 0) + COALESCE(vector_double_ops, 0) DESC,
            function_name ASC,
            file_name ASC,
            line ASC;
    ";
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

        let data = RooflineData::load(&connection, Some(calibration()));

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
    fn computes_memory_and_compute_bound_efficiency_against_calibration() {
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
        };
        let calibration = calibration();

        assert_eq!(loop_data.relevant_roof_gflops(&calibration), Some(50.0));
        assert_eq!(loop_data.efficiency(&calibration), Some(0.5));

        loop_data.scalar_double_ops = Some(100_000_000_000.0);
        loop_data.scalar_double_ai = Some(8.0);
        assert_eq!(loop_data.relevant_roof_gflops(&calibration), Some(200.0));
        assert_eq!(loop_data.efficiency(&calibration), Some(0.5));
    }

    #[test]
    fn reports_a_missing_roofline_view_without_hiding_the_tab() {
        let connection = sqlite::open(":memory:").unwrap();
        let data = RooflineData::load(&connection, Some(calibration()));

        assert!(data.loops.is_empty());
        assert!(
            data.error
                .as_deref()
                .is_some_and(|error| error.contains("roofline"))
        );
        assert!(data.calibration.is_some());
    }
}
