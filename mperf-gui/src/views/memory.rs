use gpui::{div, prelude::*, px, rgb, Div, FontWeight};

use crate::{
    memory::{MemoryData, MemorySummary},
    theme::{ACCENT, BORDER, ERROR, MUTED_TEXT, SURFACE, WORKSPACE},
    MperfGui,
};

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
            .child(
                div()
                    .h(px(42.0))
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
                            .child("Memory analysis"),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(format!(
                        "{} · {} · {}",
                        summary.bandwidth_source, summary.bandwidth_scope, summary.quality
                    ))),
            )
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
                    .child(summary_cards(summary))
                    .child(section(
                        "Multi-scale working set",
                        data.working_set
                            .iter()
                            .map(|point| {
                                format!(
                                    "{:>8} refs   mean {:>10}   p95 {:>10}   max {:>10}",
                                    point.window_references,
                                    format_bytes(point.mean_bytes as u64),
                                    format_bytes(point.p95_bytes),
                                    format_bytes(point.max_bytes)
                                )
                            })
                            .collect(),
                    ))
                    .child(section(
                        "LRU miss-ratio curve",
                        data.miss_ratio
                            .iter()
                            .filter(|point| {
                                point.cache_bytes >= 1024 && point.cache_bytes.is_power_of_two()
                            })
                            .map(|point| {
                                format!(
                                    "{:>10} cache   {:>6.2}% misses",
                                    format_bytes(point.cache_bytes),
                                    point.miss_ratio * 100.0
                                )
                            })
                            .collect(),
                    ))
                    .child(section(
                        "Spatial cache-line utilization",
                        data.spatial
                            .iter()
                            .map(|point| {
                                format!("{:>3}% utilized   {:>10} lines", point.bucket, point.count)
                            })
                            .collect(),
                    ))
                    .child(section(
                        "Stride distribution (log2 cache lines)",
                        data.strides
                            .iter()
                            .map(|point| {
                                format!(
                                    "bucket {:>4}   {:>12} references",
                                    point.bucket, point.count
                                )
                            })
                            .collect(),
                    ))
                    .child(section(
                        "Native RSS / hardware-bandwidth timeline",
                        sampled_timeline_rows(data),
                    )),
            )
    }
}

fn sampled_timeline_rows(data: &MemoryData) -> Vec<String> {
    if data.timeline.is_empty() {
        return vec!["No native timeline samples were recorded.".to_string()];
    }
    let first = data.timeline.first().map_or(0, |point| point.timestamp_ns);
    let step = (data.timeline.len() / 24).max(1);
    data.timeline
        .iter()
        .step_by(step)
        .map(|point| {
            let milliseconds = point.timestamp_ns.saturating_sub(first) as f64 / 1.0e6;
            let rss = point
                .rss_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "—".into());
            let bandwidth = point
                .read_gbytes_per_second
                .zip(point.write_gbytes_per_second)
                .map(|(read, write)| format!("R {read:.2} / W {write:.2} GB/s"))
                .unwrap_or_else(|| "—".into());
            format!("{milliseconds:>9.2} ms   RSS {rss:>10}   {bandwidth}")
        })
        .collect()
}

fn summary_cards(summary: &MemorySummary) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .child(card(
            "Accessed footprint",
            format_bytes(summary.accessed_footprint_bytes),
        ))
        .child(card(
            "Peak allocation",
            summary
                .peak_allocated_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "Unavailable".into()),
        ))
        .child(card(
            "Peak RSS",
            summary
                .peak_rss_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "Unavailable".into()),
        ))
        .child(card(
            "Cold references",
            summary
                .cold_fraction
                .map(|value| format!("{:.2}%", value * 100.0))
                .unwrap_or_else(|| "Unavailable".into()),
        ))
        .child(card(
            "Achieved bandwidth",
            summary
                .achieved_gbytes_per_second
                .map(|value| format!("{value:.2} GB/s"))
                .unwrap_or_else(|| "Unavailable".into()),
        ))
        .child(card(
            "Sustainable peak",
            summary
                .peak_gbytes_per_second
                .map(|value| format!("{value:.2} GB/s"))
                .unwrap_or_else(|| "Unavailable".into()),
        ))
        .child(card(
            "Bandwidth utilization",
            summary
                .bandwidth_utilization
                .map(|value| format!("{:.1}%", value * 100.0))
                .unwrap_or_else(|| "Unavailable".into()),
        ))
}

fn card(label: &'static str, value: String) -> Div {
    div()
        .w(px(190.0))
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
}

fn section(title: &'static str, rows: Vec<String>) -> Div {
    div()
        .p_3()
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .flex()
        .flex_col()
        .gap_1()
        .child(div().mb_1().font_weight(FontWeight::SEMIBOLD).child(title))
        .children(
            rows.into_iter()
                .map(|row| div().text_sm().text_color(rgb(MUTED_TEXT)).child(row)),
        )
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
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
