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
}

fn load(
    connection: &Connection,
    calibration_levels: Vec<MemoryLevelCalibration>,
) -> Result<MemoryData> {
    let mut statement = connection
        .prepare(
            "SELECT line_size, reference_count, architectural_load_bytes,
                architectural_store_bytes, accessed_footprint_bytes,
                modeled_dram_read_bytes, modeled_dram_write_bytes,
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
            peak_allocated_bytes: row
                .get::<_, Option<i64>>(7)?
                .map(|value| value.max(0) as u64),
            peak_rss_bytes: row
                .get::<_, Option<i64>>(8)?
                .map(|value| value.max(0) as u64),
            cold_fraction: row.get::<_, Option<f64>>(9)?,
            achieved_gbytes_per_second: row.get::<_, Option<f64>>(10)?,
            peak_gbytes_per_second: row.get::<_, Option<f64>>(11)?,
            bandwidth_utilization: row.get::<_, Option<f64>>(12)?,
            bandwidth_source: row.get::<_, String>(13)?,
            bandwidth_scope: row.get::<_, String>(14)?,
            quality: row.get::<_, String>(15)?,
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
