use anyhow::{Context, Result};
use sqlite::{Connection, State};

#[derive(Debug, Clone)]
pub struct MemoryData {
    pub summary: Option<MemorySummary>,
    pub working_set: Vec<WorkingSetPoint>,
    pub miss_ratio: Vec<MissRatioPoint>,
    pub spatial: Vec<HistogramPoint>,
    pub strides: Vec<SignedHistogramPoint>,
    pub timeline: Vec<TimelinePoint>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemorySummary {
    pub accessed_footprint_bytes: u64,
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
    pub fn load(connection: &Connection) -> Self {
        match load(connection) {
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
                error: Some(format!("{error:#}")),
            },
        }
    }
}

fn load(connection: &Connection) -> Result<MemoryData> {
    let mut statement = connection
        .prepare(
            "SELECT accessed_footprint_bytes, peak_allocated_bytes, peak_rss_bytes,
                cold_fraction, achieved_gbytes_per_second, peak_gbytes_per_second,
                bandwidth_utilization, bandwidth_source, bandwidth_scope, quality
         FROM memory_summary LIMIT 1;",
        )
        .context("memory summary is unavailable")?;
    let summary = if statement.next()? == State::Row {
        Some(MemorySummary {
            accessed_footprint_bytes: statement.read::<i64, _>(0)?.max(0) as u64,
            peak_allocated_bytes: statement
                .read::<Option<i64>, _>(1)?
                .map(|value| value.max(0) as u64),
            peak_rss_bytes: statement
                .read::<Option<i64>, _>(2)?
                .map(|value| value.max(0) as u64),
            cold_fraction: statement.read::<Option<f64>, _>(3)?,
            achieved_gbytes_per_second: statement.read::<Option<f64>, _>(4)?,
            peak_gbytes_per_second: statement.read::<Option<f64>, _>(5)?,
            bandwidth_utilization: statement.read::<Option<f64>, _>(6)?,
            bandwidth_source: statement.read::<String, _>(7)?,
            bandwidth_scope: statement.read::<String, _>(8)?,
            quality: statement.read::<String, _>(9)?,
        })
    } else {
        None
    };
    let working_set = connection.prepare(
        "SELECT window_references, mean_bytes, p95_bytes, max_bytes FROM memory_working_set ORDER BY window_references;"
    )?.into_iter().map(|row| {
        let row = row?;
        Ok(WorkingSetPoint { window_references: row.read::<i64, _>(0).max(0) as u64, mean_bytes: row.read::<f64, _>(1), p95_bytes: row.read::<i64, _>(2).max(0) as u64, max_bytes: row.read::<i64, _>(3).max(0) as u64 })
    }).collect::<Result<Vec<_>>>()?;
    let miss_ratio = connection
        .prepare("SELECT cache_bytes, miss_ratio FROM memory_miss_ratio ORDER BY cache_bytes;")?
        .into_iter()
        .map(|row| {
            let row = row?;
            Ok(MissRatioPoint {
                cache_bytes: row.read::<i64, _>(0).max(0) as u64,
                miss_ratio: row.read::<f64, _>(1),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let spatial = load_unsigned_histogram(
        connection,
        "SELECT utilization_percent, lines FROM memory_spatial_utilization ORDER BY utilization_percent;",
    )?;
    let strides = connection
        .prepare("SELECT stride_log2_lines, reference_count FROM memory_strides ORDER BY stride_log2_lines;")?
        .into_iter()
        .map(|row| {
            let row = row?;
            Ok(SignedHistogramPoint { bucket: row.read::<i64, _>(0), count: row.read::<i64, _>(1).max(0) as u64 })
        })
        .collect::<Result<Vec<_>>>()?;
    let timeline = connection
        .prepare(
            "SELECT timestamp_ns, rss_bytes, dram_read_gbytes_per_second, dram_write_gbytes_per_second
             FROM memory_timeline
             WHERE rss_bytes IS NOT NULL OR dram_read_gbytes_per_second IS NOT NULL
             ORDER BY timestamp_ns;",
        )?
        .into_iter()
        .map(|row| {
            let row = row?;
            Ok(TimelinePoint {
                timestamp_ns: row.read::<i64, _>(0).max(0) as u64,
                rss_bytes: row
                    .read::<Option<i64>, _>(1)
                    .map(|value| value.max(0) as u64),
                read_gbytes_per_second: row.read::<Option<f64>, _>(2),
                write_gbytes_per_second: row.read::<Option<f64>, _>(3),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(MemoryData {
        summary,
        working_set,
        miss_ratio,
        spatial,
        strides,
        timeline,
        error: None,
    })
}

fn load_unsigned_histogram(connection: &Connection, query: &str) -> Result<Vec<HistogramPoint>> {
    connection
        .prepare(query)?
        .into_iter()
        .map(|row| {
            let row = row?;
            Ok(HistogramPoint {
                bucket: row.read::<i64, _>(0).max(0) as u64,
                count: row.read::<i64, _>(1).max(0) as u64,
            })
        })
        .collect()
}
