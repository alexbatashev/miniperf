use anyhow::{Context, Result};
use mperf_data::MemoryLevelCalibration;

use crate::sql::{Connection, SqlResult};

#[derive(Debug, Clone)]
pub struct MemoryData {
    pub summary: Option<MemorySummary>,
    pub working_set: Vec<WorkingSetPoint>,
    pub miss_ratio: Vec<MissRatioPoint>,
    pub spatial: Vec<HistogramPoint>,
    pub strides: Vec<SignedHistogramPoint>,
    pub timeline: Vec<TimelinePoint>,
    pub calibration_levels: Vec<MemoryLevelCalibration>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemorySummary {
    pub line_size: u64,
    pub reference_count: u64,
    pub architectural_load_bytes: u64,
    pub architectural_store_bytes: u64,
    pub accessed_footprint_bytes: u64,
    pub modeled_dram_read_bytes: u64,
    pub modeled_dram_write_bytes: u64,
    pub native_duration_ns: u64,
    pub peak_allocated_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub cold_fraction: Option<f64>,
    pub achieved_gbytes_per_second: Option<f64>,
    pub peak_gbytes_per_second: Option<f64>,
    pub bandwidth_utilization: Option<f64>,
    pub bandwidth_source: String,
    pub bandwidth_scope: String,
    pub quality: String,
}

#[derive(Debug, Clone)]
pub struct WorkingSetPoint {
    pub window_references: u64,
    pub mean_bytes: f64,
    pub p95_bytes: u64,
    pub max_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct MissRatioPoint {
    pub cache_bytes: u64,
    pub miss_ratio: f64,
}
#[derive(Debug, Clone)]
pub struct HistogramPoint {
    pub bucket: u64,
    pub count: u64,
}
#[derive(Debug, Clone)]
pub struct SignedHistogramPoint {
    pub bucket: i64,
    pub count: u64,
}
#[derive(Debug, Clone)]
pub struct TimelinePoint {
    pub timestamp_ns: u64,
    pub rss_bytes: Option<u64>,
    pub read_gbytes_per_second: Option<f64>,
    pub write_gbytes_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CacheTrafficLevel {
    pub label: String,
    pub capacity_bytes: u64,
    pub shared_by: usize,
    pub miss_ratio: f64,
    pub line_fill_bytes: f64,
    pub bandwidth_gbytes_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHierarchy {
    pub levels: Vec<CacheTrafficLevel>,
    pub uses_recorded_topology: bool,
}

impl MemoryData {
    pub fn load(connection: &Connection, calibration_levels: Vec<MemoryLevelCalibration>) -> Self {
        match load(connection, calibration_levels) {
            Ok(mut data) => {
                data.error = None;
                data
            }
            Err(error) => Self {
                summary: None,
                working_set: Vec::new(),
                miss_ratio: Vec::new(),
                spatial: Vec::new(),
                strides: Vec::new(),
                timeline: Vec::new(),
                calibration_levels: Vec::new(),
                error: Some(format!("{error:#}")),
            },
        }
    }

    pub fn hierarchy(&self) -> Option<MemoryHierarchy> {
        let summary = self.summary.as_ref()?;
        let recorded = self
            .calibration_levels
            .iter()
            .filter(|level| level.capacity_bytes > 0)
            .map(|level| {
                (
                    level.level.clone(),
                    level.capacity_bytes,
                    level.shared_by,
                    Some(level.gbytes_per_second),
                )
            })
            .collect::<Vec<_>>();
        let uses_recorded_topology = !recorded.is_empty();
        let definitions = if uses_recorded_topology {
            recorded
        } else {
            vec![
                ("32 KiB cache".to_string(), 32 * 1024, 0, None),
                ("256 KiB cache".to_string(), 256 * 1024, 0, None),
                ("8 MiB cache".to_string(), 8 * 1024 * 1024, 0, None),
            ]
        };
        let levels = definitions
            .into_iter()
            .map(
                |(label, capacity_bytes, shared_by, bandwidth_gbytes_per_second)| {
                    let miss_ratio = self.miss_ratio_at(capacity_bytes);
                    CacheTrafficLevel {
                        label,
                        capacity_bytes,
                        shared_by,
                        miss_ratio,
                        line_fill_bytes: miss_ratio
                            * summary.reference_count as f64
                            * summary.line_size as f64,
                        bandwidth_gbytes_per_second,
                    }
                },
            )
            .collect();
        Some(MemoryHierarchy {
            levels,
            uses_recorded_topology,
        })
    }

    fn miss_ratio_at(&self, capacity_bytes: u64) -> f64 {
        self.miss_ratio
            .iter()
            .min_by_key(|point| point.cache_bytes.abs_diff(capacity_bytes))
            .map_or(0.0, |point| point.miss_ratio.clamp(0.0, 1.0))
    }
}

fn load(
    connection: &Connection,
    calibration_levels: Vec<MemoryLevelCalibration>,
) -> Result<MemoryData> {
    let mut statement = connection
        .prepare(
            "SELECT line_size, reference_count, architectural_load_bytes,
                architectural_store_bytes, accessed_footprint_bytes,
                modeled_dram_read_bytes, modeled_dram_write_bytes, native_duration_ns,
                peak_allocated_bytes, peak_rss_bytes,
                cold_fraction, achieved_gbytes_per_second, peak_gbytes_per_second,
                bandwidth_utilization, bandwidth_source, bandwidth_scope, quality
         FROM memory_summary LIMIT 1;",
        )
        .context("memory summary is unavailable")?;
    let summary = match statement.query([])?.next()? {
        Some(row) => Some(MemorySummary {
            line_size: row.get::<_, i64>(0)?.max(0) as u64,
            reference_count: row.get::<_, i64>(1)?.max(0) as u64,
            architectural_load_bytes: row.get::<_, i64>(2)?.max(0) as u64,
            architectural_store_bytes: row.get::<_, i64>(3)?.max(0) as u64,
            accessed_footprint_bytes: row.get::<_, i64>(4)?.max(0) as u64,
            modeled_dram_read_bytes: row.get::<_, i64>(5)?.max(0) as u64,
            modeled_dram_write_bytes: row.get::<_, i64>(6)?.max(0) as u64,
            native_duration_ns: row.get::<_, i64>(7)?.max(0) as u64,
            peak_allocated_bytes: row
                .get::<_, Option<i64>>(8)?
                .map(|value| value.max(0) as u64),
            peak_rss_bytes: row
                .get::<_, Option<i64>>(9)?
                .map(|value| value.max(0) as u64),
            cold_fraction: row.get::<_, Option<f64>>(10)?,
            achieved_gbytes_per_second: row.get::<_, Option<f64>>(11)?,
            peak_gbytes_per_second: row.get::<_, Option<f64>>(12)?,
            bandwidth_utilization: row.get::<_, Option<f64>>(13)?,
            bandwidth_source: row.get::<_, String>(14)?,
            bandwidth_scope: row.get::<_, String>(15)?,
            quality: row.get::<_, String>(16)?,
        }),
        None => None,
    };
    let mut statement = connection.prepare(
        "SELECT window_references, mean_bytes, p95_bytes, max_bytes FROM memory_working_set ORDER BY window_references;",
    )?;
    let working_set = statement
        .query_map([], |row| {
            Ok(WorkingSetPoint {
                window_references: row.get::<_, i64>(0)?.max(0) as u64,
                mean_bytes: row.get::<_, f64>(1)?,
                p95_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                max_bytes: row.get::<_, i64>(3)?.max(0) as u64,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    let mut statement = connection
        .prepare("SELECT cache_bytes, miss_ratio FROM memory_miss_ratio ORDER BY cache_bytes;")?;
    let miss_ratio = statement
        .query_map([], |row| {
            Ok(MissRatioPoint {
                cache_bytes: row.get::<_, i64>(0)?.max(0) as u64,
                miss_ratio: row.get::<_, f64>(1)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    let spatial = load_unsigned_histogram(
        connection,
        "SELECT utilization_percent, lines FROM memory_spatial_utilization ORDER BY utilization_percent;",
    )?;
    let mut statement = connection.prepare(
        "SELECT stride_log2_lines, reference_count FROM memory_strides ORDER BY stride_log2_lines;",
    )?;
    let strides = statement
        .query_map([], |row| {
            Ok(SignedHistogramPoint {
                bucket: row.get::<_, i64>(0)?,
                count: row.get::<_, i64>(1)?.max(0) as u64,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    let mut statement = connection.prepare(
        "SELECT timestamp_ns, rss_bytes, dram_read_gbytes_per_second, dram_write_gbytes_per_second
             FROM memory_timeline
             WHERE rss_bytes IS NOT NULL OR dram_read_gbytes_per_second IS NOT NULL
             ORDER BY timestamp_ns;",
    )?;
    let timeline = statement
        .query_map([], |row| {
            Ok(TimelinePoint {
                timestamp_ns: row.get::<_, i64>(0)?.max(0) as u64,
                rss_bytes: row
                    .get::<_, Option<i64>>(1)?
                    .map(|value| value.max(0) as u64),
                read_gbytes_per_second: row.get::<_, Option<f64>>(2)?,
                write_gbytes_per_second: row.get::<_, Option<f64>>(3)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    Ok(MemoryData {
        summary,
        working_set,
        miss_ratio,
        spatial,
        strides,
        timeline,
        calibration_levels,
        error: None,
    })
}

fn load_unsigned_histogram(connection: &Connection, query: &str) -> Result<Vec<HistogramPoint>> {
    let mut statement = connection.prepare(query)?;
    let points = statement
        .query_map([], |row| {
            Ok(HistogramPoint {
                bucket: row.get::<_, i64>(0)?.max(0) as u64,
                count: row.get::<_, i64>(1)?.max(0) as u64,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_uses_recorded_capacities_for_lru_line_fill_traffic() {
        let data = MemoryData {
            summary: Some(MemorySummary {
                line_size: 64,
                reference_count: 100,
                architectural_load_bytes: 800,
                architectural_store_bytes: 400,
                accessed_footprint_bytes: 64 * 1024,
                modeled_dram_read_bytes: 1_024,
                modeled_dram_write_bytes: 512,
                native_duration_ns: 1_000,
                peak_allocated_bytes: None,
                peak_rss_bytes: None,
                cold_fraction: Some(0.1),
                achieved_gbytes_per_second: Some(1.0),
                peak_gbytes_per_second: Some(10.0),
                bandwidth_utilization: Some(0.1),
                bandwidth_source: "process_modeled".to_string(),
                bandwidth_scope: "process".to_string(),
                quality: "test".to_string(),
            }),
            working_set: Vec::new(),
            miss_ratio: vec![MissRatioPoint {
                cache_bytes: 32 * 1024,
                miss_ratio: 0.25,
            }],
            spatial: Vec::new(),
            strides: Vec::new(),
            timeline: Vec::new(),
            calibration_levels: vec![MemoryLevelCalibration {
                level: "L1".to_string(),
                gbytes_per_second: 200.0,
                gbytes_per_second_samples: vec![200.0],
                working_set_bytes: 16 * 1024,
                capacity_bytes: 32 * 1024,
                shared_by: 1,
            }],
            error: None,
        };

        let hierarchy = data.hierarchy().unwrap();
        assert!(hierarchy.uses_recorded_topology);
        assert_eq!(hierarchy.levels.len(), 1);
        assert_eq!(hierarchy.levels[0].label, "L1");
        assert_eq!(hierarchy.levels[0].capacity_bytes, 32 * 1024);
        assert_eq!(hierarchy.levels[0].shared_by, 1);
        assert_eq!(hierarchy.levels[0].miss_ratio, 0.25);
        assert_eq!(hierarchy.levels[0].line_fill_bytes, 1_600.0);
        assert_eq!(hierarchy.levels[0].bandwidth_gbytes_per_second, Some(200.0));
    }
}
