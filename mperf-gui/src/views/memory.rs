use gpui::{Div, FontWeight, div, prelude::*, px, relative, rgb};

use crate::{
    MperfGui,
    memory::{CacheTrafficLevel, MemoryData, MemorySummary},
    theme::{ACCENT, BORDER, CHROME, ERROR, MUTED_TEXT, SURFACE, TEXT, WORKSPACE},
};

const READ_COLOR: u32 = 0x6d86b3;
const WRITE_COLOR: u32 = 0xa66f68;
const GOOD: u32 = 0x5f966e;
const WARNING: u32 = 0xc59654;

impl MperfGui {
    pub(crate) fn render_memory_workspace(&self) -> Div {
        let Some(data) = self.memory_data() else {
            return message("This recording does not contain memory analysis.", false);
        };
        if let Some(error) = data.error.as_deref() {
            return message(error, true);
        }
        let Some(summary) = data.summary.as_ref() else {
            return message("The memory recording has no summary row.", true);
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(memory_header(summary))
            .child(
                div()
                    .id("memory-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .flex()
                    .flex_col()
                    .child(overview(summary, data))
                    .child(hierarchy_section(summary, data))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .child(working_set_section(data))
                            .child(miss_ratio_section(summary, data)),
                    )
                    .child(access_pattern_section(data))
                    .child(timeline_section(data)),
            )
    }
}

fn memory_header(summary: &MemorySummary) -> Div {
    let method = if summary.bandwidth_source == "hardware_memory_controller" {
        "hardware controller bandwidth"
    } else {
        "modeled process bandwidth"
    };
    let scope = if summary.bandwidth_scope == "system_during_target" {
        "system-scoped"
    } else {
        "process-scoped"
    };

    div()
        .h(px(42.0))
        .min_h(px(42.0))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Memory behavior"),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(format!("{method} · {scope}")),
        )
        .child(div().flex_1())
        .child(
            div()
                .rounded_sm()
                .px_2()
                .py_1()
                .bg(rgb(CHROME))
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(summary.quality.clone()),
        )
}

fn overview(summary: &MemorySummary, data: &MemoryData) -> Div {
    let utilization = summary
        .bandwidth_utilization
        .unwrap_or_default()
        .clamp(0.0, 1.0);
    let diagnosis = bandwidth_diagnosis(summary, data);

    div()
        .flex()
        .flex_wrap()
        .gap_3()
        .child(
            panel()
                .w(px(390.0))
                .min_w(px(330.0))
                .child(section_title("DRAM bandwidth", diagnosis.0))
                .child(
                    div()
                        .px_3()
                        .pt_3()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(
                            div()
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(diagnosis.2))
                                .child(
                                    summary
                                        .achieved_gbytes_per_second
                                        .map(|value| format!("{value:.2} GB/s"))
                                        .unwrap_or_else(|| "Unavailable".to_string()),
                                ),
                        )
                        .child(
                            div().pb_1().text_xs().text_color(rgb(MUTED_TEXT)).child(
                                summary
                                    .peak_gbytes_per_second
                                    .map(|peak| format!("of {peak:.2} GB/s sustainable"))
                                    .unwrap_or_else(|| "without a recorded ceiling".to_string()),
                            ),
                        ),
                )
                .child(
                    div()
                        .mx_3()
                        .mt_2()
                        .h(px(9.0))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(CHROME))
                        .child(
                            div()
                                .h_full()
                                .w(relative(utilization as f32))
                                .bg(rgb(diagnosis.2)),
                        ),
                )
                .child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child(diagnosis.1),
                ),
        )
        .child(
            div()
                .min_w(px(620.0))
                .flex_1()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(metric_card(
                    "Modeled DRAM traffic",
                    format_bytes(
                        summary
                            .modeled_dram_read_bytes
                            .saturating_add(summary.modeled_dram_write_bytes),
                    ),
                    format!(
                        "{} read · {} write over {}",
                        format_bytes(summary.modeled_dram_read_bytes),
                        format_bytes(summary.modeled_dram_write_bytes),
                        format_duration_ns(summary.native_duration_ns)
                    ),
                ))
                .child(metric_card(
                    "Architectural requests",
                    format_bytes(
                        summary
                            .architectural_load_bytes
                            .saturating_add(summary.architectural_store_bytes),
                    ),
                    format!(
                        "{} load · {} store",
                        format_bytes(summary.architectural_load_bytes),
                        format_bytes(summary.architectural_store_bytes)
                    ),
                ))
                .child(metric_card(
                    "Accessed footprint",
                    format_bytes(summary.accessed_footprint_bytes),
                    format!(
                        "{} memory references",
                        format_count(summary.reference_count)
                    ),
                ))
                .child(metric_card(
                    "Peak allocation / RSS",
                    summary
                        .peak_allocated_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "Unavailable".to_string()),
                    format!(
                        "{} peak RSS",
                        summary
                            .peak_rss_bytes
                            .map(format_bytes)
                            .unwrap_or_else(|| "unavailable".to_string())
                    ),
                ))
                .child(metric_card(
                    "First-touch references",
                    summary
                        .cold_fraction
                        .map(|value| format!("{:.3}%", value * 100.0))
                        .unwrap_or_else(|| "Unavailable".to_string()),
                    "Cold lines with no prior reuse".to_string(),
                )),
        )
}

fn bandwidth_diagnosis(summary: &MemorySummary, data: &MemoryData) -> (&'static str, String, u32) {
    let utilization = summary.bandwidth_utilization;
    let spatial = average_spatial_utilization(data);
    match utilization {
        Some(value) if value >= 0.8 => (
            "Bandwidth saturated",
            "DRAM traffic is close to the calibrated sustainable ceiling. Reduce memory traffic or increase locality before chasing instruction-level effects."
                .to_string(),
            ERROR,
        ),
        Some(value) if value >= 0.5 => (
            "Bandwidth pressure",
            "The workload consumes a material share of sustainable DRAM bandwidth. Inspect the hierarchy traffic and cache-line utilization below."
                .to_string(),
            WARNING,
        ),
        Some(_) if spatial.is_some_and(|value| value < 0.5) => (
            "Latency / locality candidate",
            "DRAM bandwidth has headroom, but cache lines are poorly utilized. The likely opportunity is access locality or data layout, not raw bandwidth."
                .to_string(),
            WARNING,
        ),
        Some(_) => (
            "Bandwidth headroom",
            "The workload is well below the sustainable DRAM ceiling. Use cache retention and per-function LLC metrics to locate latency or code bottlenecks."
                .to_string(),
            GOOD,
        ),
        None => (
            "No calibrated ceiling",
            "Bandwidth was not measured against a sustainable host calibration. Traffic and locality views remain available."
                .to_string(),
            MUTED_TEXT,
        ),
    }
}

fn hierarchy_section(summary: &MemorySummary, data: &MemoryData) -> Div {
    let Some(hierarchy) = data.hierarchy() else {
        return panel().child(section_title(
            "Memory hierarchy traffic",
            "No reuse-distance data was recorded",
        ));
    };
    let topology_note = if hierarchy.uses_recorded_topology {
        "Source-host cache capacities · LRU-modeled line fills"
    } else {
        "Generic cache capacities · re-record to preserve source-host topology"
    };

    panel()
        .child(section_title("Memory hierarchy traffic", topology_note))
        .child(hierarchy_header())
        .children(hierarchy.levels.iter().map(hierarchy_row))
        .child(dram_row(summary))
        .child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(
                    "Cache rows estimate demand line fills from the exact-address reuse-distance stream. DRAM read/write is the recorder's LLC traffic model; hardware-controller bandwidth is system-scoped and shown separately.",
                ),
        )
}

fn hierarchy_header() -> Div {
    div()
        .h(px(24.0))
        .flex()
        .items_center()
        .bg(rgb(CHROME))
        .border_t_1()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED_TEXT))
        .child(div().w(px(160.0)).px_3().child("Level"))
        .child(div().w(px(150.0)).text_right().child("Capacity / sharing"))
        .child(
            div()
                .min_w(px(220.0))
                .flex_1()
                .px_3()
                .child("Cache retention"),
        )
        .child(
            div()
                .w(px(170.0))
                .text_right()
                .px_3()
                .child("Line fills outward"),
        )
        .child(
            div()
                .w(px(150.0))
                .text_right()
                .px_3()
                .child("Measured roof"),
        )
}

fn hierarchy_row(level: &CacheTrafficLevel) -> Div {
    let hit_ratio = (1.0 - level.miss_ratio).clamp(0.0, 1.0);
    div()
        .h(px(42.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .w(px(160.0))
                .px_3()
                .font_weight(FontWeight::SEMIBOLD)
                .child(level.label.clone()),
        )
        .child(
            div()
                .w(px(150.0))
                .text_right()
                .text_color(rgb(MUTED_TEXT))
                .child(if level.shared_by > 0 {
                    format!(
                        "{} · {} CPU{}",
                        format_bytes(level.capacity_bytes),
                        level.shared_by,
                        if level.shared_by == 1 { "" } else { "s" }
                    )
                } else {
                    format_bytes(level.capacity_bytes)
                }),
        )
        .child(
            div()
                .min_w(px(220.0))
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .child(
                    div()
                        .h(px(8.0))
                        .min_w(px(120.0))
                        .flex_1()
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(CHROME))
                        .child(div().h_full().w(relative(hit_ratio as f32)).bg(rgb(GOOD))),
                )
                .child(
                    div()
                        .w(px(92.0))
                        .text_right()
                        .text_sm()
                        .child(format!("{:.2}% hit", hit_ratio * 100.0)),
                ),
        )
        .child(
            div()
                .w(px(170.0))
                .text_right()
                .px_3()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format_bytes(level.line_fill_bytes.max(0.0) as u64)),
        )
        .child(
            div()
                .w(px(150.0))
                .text_right()
                .px_3()
                .text_color(rgb(MUTED_TEXT))
                .child(
                    level
                        .bandwidth_gbytes_per_second
                        .map(|value| format!("{value:.1} GB/s"))
                        .unwrap_or_else(|| "—".to_string()),
                ),
        )
}

fn dram_row(summary: &MemorySummary) -> Div {
    div()
        .h(px(46.0))
        .flex()
        .items_center()
        .bg(rgb(CHROME))
        .child(
            div()
                .w(px(310.0))
                .px_3()
                .font_weight(FontWeight::SEMIBOLD)
                .child("DRAM / memory controller"),
        )
        .child(
            div()
                .min_w(px(220.0))
                .flex_1()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .child(legend(
                    READ_COLOR,
                    format!("{} read", format_bytes(summary.modeled_dram_read_bytes)),
                ))
                .child(legend(
                    WRITE_COLOR,
                    format!("{} write", format_bytes(summary.modeled_dram_write_bytes)),
                )),
        )
        .child(
            div()
                .w(px(170.0))
                .text_right()
                .px_3()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format_bytes(
                    summary
                        .modeled_dram_read_bytes
                        .saturating_add(summary.modeled_dram_write_bytes),
                )),
        )
        .child(
            div()
                .w(px(150.0))
                .text_right()
                .px_3()
                .text_color(rgb(MUTED_TEXT))
                .child(
                    summary
                        .peak_gbytes_per_second
                        .map(|value| format!("{value:.1} GB/s"))
                        .unwrap_or_else(|| "—".to_string()),
                ),
        )
}

fn working_set_section(data: &MemoryData) -> Div {
    let maximum = data
        .working_set
        .iter()
        .map(|point| point.max_bytes)
        .max()
        .unwrap_or(1)
        .max(1);
    panel()
        .min_w(px(600.0))
        .flex_1()
        .child(section_title(
            "Working-set growth",
            "p95 and maximum distinct bytes in each reference window",
        ))
        .children(data.working_set.iter().map(|point| {
            let p95 = point.p95_bytes as f64 / maximum as f64;
            let max = point.max_bytes as f64 / maximum as f64;
            div()
                .h(px(34.0))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .w(px(120.0))
                        .px_3()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child(format!("{} refs", format_count(point.window_references))),
                )
                .child(
                    div()
                        .min_w(px(180.0))
                        .flex_1()
                        .h(px(12.0))
                        .relative()
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(CHROME))
                        .child(div().h_full().w(relative(max as f32)).bg(rgb(0x4b4b50)))
                        .child(
                            div()
                                .absolute()
                                .h_full()
                                .w(relative(p95 as f32))
                                .bg(rgb(ACCENT)),
                        ),
                )
                .child(
                    div()
                        .w(px(280.0))
                        .px_3()
                        .text_right()
                        .text_xs()
                        .child(format!(
                            "mean {} · p95 {} · max {}",
                            format_bytes(point.mean_bytes.max(0.0) as u64),
                            format_bytes(point.p95_bytes),
                            format_bytes(point.max_bytes)
                        )),
                )
        }))
}

fn miss_ratio_section(summary: &MemorySummary, data: &MemoryData) -> Div {
    let upper = summary
        .accessed_footprint_bytes
        .saturating_mul(4)
        .max(1024 * 1024);
    let points = data
        .miss_ratio
        .iter()
        .filter(|point| {
            point.cache_bytes >= 1024
                && point.cache_bytes <= upper
                && point.cache_bytes.is_power_of_two()
        })
        .collect::<Vec<_>>();
    panel()
        .min_w(px(520.0))
        .flex_1()
        .child(section_title(
            "Cache retention curve",
            "LRU-modeled demand misses by cache capacity",
        ))
        .children(points.into_iter().map(|point| {
            let miss = point.miss_ratio.clamp(0.0, 1.0);
            div()
                .h(px(28.0))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .w(px(110.0))
                        .px_3()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child(format_bytes(point.cache_bytes)),
                )
                .child(
                    div()
                        .min_w(px(160.0))
                        .flex_1()
                        .h(px(8.0))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(CHROME))
                        .child(div().h_full().w(relative(miss as f32)).bg(rgb(WARNING))),
                )
                .child(
                    div()
                        .w(px(100.0))
                        .px_3()
                        .text_right()
                        .text_xs()
                        .child(format!("{:.2}% miss", miss * 100.0)),
                )
        }))
}

fn access_pattern_section(data: &MemoryData) -> Div {
    let spatial_total = data
        .spatial
        .iter()
        .map(|point| point.count)
        .sum::<u64>()
        .max(1);
    let stride_total = data
        .strides
        .iter()
        .map(|point| point.count)
        .sum::<u64>()
        .max(1);
    let spatial_average = average_spatial_utilization(data)
        .map(|value| format!("{:.1}% average line use", value * 100.0))
        .unwrap_or_else(|| "No spatial samples".to_string());
    let mut strides = data.strides.iter().collect::<Vec<_>>();
    strides.sort_by_key(|point| std::cmp::Reverse(point.count));

    panel()
        .child(section_title("Access pattern", spatial_average))
        .child(
            div()
                .flex()
                .flex_wrap()
                .child(
                    div()
                        .min_w(px(520.0))
                        .flex_1()
                        .p_3()
                        .border_r_1()
                        .border_color(rgb(BORDER))
                        .child(
                            div()
                                .mb_2()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(MUTED_TEXT))
                                .child("CACHE-LINE UTILIZATION"),
                        )
                        .child(
                            div()
                                .h(px(28.0))
                                .flex()
                                .rounded_sm()
                                .overflow_hidden()
                                .bg(rgb(CHROME))
                                .children(data.spatial.iter().map(|point| {
                                    let fraction = point.count as f64 / spatial_total as f64;
                                    let color = utilization_color(point.bucket as f64 / 100.0);
                                    div().h_full().w(relative(fraction as f32)).bg(rgb(color))
                                })),
                        )
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child("0% used"))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED_TEXT))
                                        .child("100% used"),
                                ),
                        ),
                )
                .child(
                    div()
                        .min_w(px(520.0))
                        .flex_1()
                        .p_3()
                        .child(
                            div()
                                .mb_2()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(MUTED_TEXT))
                                .child("DOMINANT STRIDES"),
                        )
                        .children(strides.into_iter().take(6).map(|point| {
                            let fraction = point.count as f64 / stride_total as f64;
                            div()
                                .h(px(24.0))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(110.0))
                                        .text_xs()
                                        .text_color(rgb(MUTED_TEXT))
                                        .child(stride_label(point.bucket)),
                                )
                                .child(
                                    div()
                                        .min_w(px(120.0))
                                        .flex_1()
                                        .h(px(7.0))
                                        .rounded_sm()
                                        .overflow_hidden()
                                        .bg(rgb(CHROME))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(fraction as f32))
                                                .bg(rgb(ACCENT)),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(72.0))
                                        .text_right()
                                        .text_xs()
                                        .child(format!("{:.1}%", fraction * 100.0)),
                                )
                        })),
                ),
        )
}

fn timeline_section(data: &MemoryData) -> Div {
    let bandwidth = data
        .timeline
        .iter()
        .filter_map(|point| {
            Some((
                point.timestamp_ns,
                point.read_gbytes_per_second?,
                point.write_gbytes_per_second?,
            ))
        })
        .collect::<Vec<_>>();
    if bandwidth.is_empty() {
        return panel().child(section_title(
            "Bandwidth over time",
            "No hardware-controller timeline samples were recorded",
        ));
    }
    let first = bandwidth.first().map_or(0, |point| point.0);
    let last = bandwidth.last().map_or(first, |point| point.0);
    let peak_rss = data
        .timeline
        .iter()
        .filter_map(|point| point.rss_bytes)
        .max();
    let step = (bandwidth.len() / 64).max(1);
    let sampled = bandwidth.iter().step_by(step).collect::<Vec<_>>();
    let maximum = sampled
        .iter()
        .map(|(_, read, write)| read + write)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);

    panel()
        .child(section_title(
            "Bandwidth over time",
            format!(
                "{} samples over {} · peak RSS {} · blue read / red write",
                bandwidth.len(),
                format_duration_ns(last.saturating_sub(first)),
                peak_rss
                    .map(format_bytes)
                    .unwrap_or_else(|| "unavailable".to_string())
            ),
        ))
        .child(
            div()
                .h(px(150.0))
                .p_3()
                .flex()
                .items_end()
                .gap(px(2.0))
                .children(sampled.into_iter().map(|(_, read, write)| {
                    let total = read + write;
                    let height = (total / maximum).clamp(0.0, 1.0);
                    let read_share = if total > 0.0 { read / total } else { 0.0 };
                    div()
                        .min_w(px(4.0))
                        .flex_1()
                        .h(relative(height as f32))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .bg(rgb(WRITE_COLOR))
                        .child(
                            div()
                                .w_full()
                                .h(relative(read_share as f32))
                                .bg(rgb(READ_COLOR)),
                        )
                })),
        )
}

fn panel() -> Div {
    div()
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .overflow_hidden()
}

fn section_title(title: impl Into<String>, subtitle: impl Into<String>) -> Div {
    div()
        .min_h(px(42.0))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .bg(rgb(SURFACE))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(title.into()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(subtitle.into()),
        )
}

fn metric_card(label: &'static str, value: String, detail: String) -> Div {
    div()
        .min_w(px(245.0))
        .flex_1()
        .p_3()
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(label))
        .child(
            div()
                .mt_1()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(ACCENT))
                .child(value),
        )
        .child(
            div()
                .mt_1()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(detail),
        )
}

fn legend(color: u32, label: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .child(div().w(px(8.0)).h(px(8.0)).rounded_sm().bg(rgb(color)))
        .child(label)
}

fn average_spatial_utilization(data: &MemoryData) -> Option<f64> {
    let count = data.spatial.iter().map(|point| point.count).sum::<u64>();
    (count > 0).then(|| {
        data.spatial
            .iter()
            .map(|point| point.bucket as f64 * point.count as f64)
            .sum::<f64>()
            / count as f64
            / 100.0
    })
}

fn utilization_color(value: f64) -> u32 {
    if value >= 0.75 {
        GOOD
    } else if value >= 0.4 {
        WARNING
    } else {
        ERROR
    }
}

fn stride_label(bucket: i64) -> String {
    match bucket {
        i64::MIN..=-1 => "same cache line".to_string(),
        0 => "1 cache line".to_string(),
        1 => "2 cache lines".to_string(),
        value if value < 63 => format!("{} cache lines", 1_u64 << value),
        _ => "very large".to_string(),
    }
}

fn format_count(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.2}B", value as f64 / 1.0e9)
    } else if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1.0e6)
    } else if value >= 1_000 {
        format!("{:.2}K", value as f64 / 1.0e3)
    } else {
        value.to_string()
    }
}

fn format_duration_ns(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.2} s", value as f64 / 1.0e9)
    } else if value >= 1_000_000 {
        format!("{:.2} ms", value as f64 / 1.0e6)
    } else if value >= 1_000 {
        format!("{:.1} µs", value as f64 / 1.0e3)
    } else {
        format!("{value} ns")
    }
}

fn message(text: impl Into<String>, error: bool) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(WORKSPACE))
        .child(
            div()
                .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
                .child(text.into()),
        )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
