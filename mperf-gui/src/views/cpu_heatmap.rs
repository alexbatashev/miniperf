use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, ScrollHandle, SharedString,
    canvas, div, fill, point, prelude::*, px, rgb, size,
};

use crate::{
    MperfGui,
    profile::{CpuObservationSource, TimeRange},
    profile_analysis::CpuUtilizationHeatmap,
    theme::{ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SURFACE, TEXT, WORKSPACE},
};

const AUTO_MAX_BUCKETS: usize = 240;
const MAX_EXPLICIT_BUCKETS: u64 = 20_000;
const LANE_HEIGHT: f32 = 22.0;
const MIN_GRID_HEIGHT: f32 = 180.0;
const MIN_GRID_WIDTH: f32 = 920.0;
const MAX_GRID_WIDTH: f32 = 80_000.0;
const LANE_LABEL_WIDTH: f32 = 132.0;
const CELL_TARGET_WIDTH: f32 = 11.0;

const BUCKET_PRESETS: [(&str, u64); 7] = [
    ("10 ms", 10_000_000),
    ("50 ms", 50_000_000),
    ("100 ms", 100_000_000),
    ("1 s", 1_000_000_000),
    ("2 s", 2_000_000_000),
    ("5 s", 5_000_000_000),
    ("10 s", 10_000_000_000),
];

impl MperfGui {
    pub(crate) fn render_cpu_heatmap_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return self.render_cpu_heatmap_message(
                "Open a recording to inspect CPU utilization.",
                false,
                cx,
            );
        };
        let profile = &model.profile;
        if let Some(error) = profile.error.clone() {
            return self.render_cpu_heatmap_message(error, true, cx);
        }

        let filter = self.profile_sample_filter();
        if let Some(bucket_ns) = self.cpu_heatmap_bucket_ns {
            let range = filter.range.or_else(|| profile.full_range());
            if range.is_some_and(|range| {
                div_ceil(range.end_ns.saturating_sub(range.start_ns), bucket_ns)
                    > MAX_EXPLICIT_BUCKETS
            }) {
                return self.render_cpu_heatmap_message(
                    format!(
                        "The selected {} bucket would create more than {} columns. \
                         Choose Auto or a coarser bucket to keep the profiler responsive.",
                        format_duration(bucket_ns),
                        MAX_EXPLICIT_BUCKETS
                    ),
                    false,
                    cx,
                );
            }
        }

        let heatmap = self.cached_cpu_heatmap(AUTO_MAX_BUCKETS, self.cpu_heatmap_bucket_ns);
        let Some(heatmap) = heatmap else {
            let message = if !profile.cpu_observations.is_empty() {
                "CPU activity has no observations matching the current global filters. \
                 Clear or adjust the filter to restore the quantitative heatmap."
            } else if profile
                .counter_metrics
                .iter()
                .any(|metric| metric.key.eq_ignore_ascii_case("os_cpu_clock"))
            {
                "CPU utilization is unavailable for the current filters because \
                 `os_cpu_clock` contains no usable values. Sample density is not \
                 a substitute for CPU utilization."
            } else {
                "CPU utilization is unavailable because this recording does not \
                 contain `os_cpu_clock`. Sample density is not shown as utilization."
            };
            return self.render_cpu_heatmap_message(message, false, cx);
        };

        let lane_count = heatmap.lanes.len();
        let effective_lane_height =
            (MIN_GRID_HEIGHT / lane_count.max(1) as f32).clamp(LANE_HEIGHT, 42.0);
        let grid_height = lane_count as f32 * effective_lane_height;
        let grid_width = cpu_grid_width(heatmap.buckets);
        let (title, semantics) = match heatmap.source {
            CpuObservationSource::CounterDelta => ("CPU utilization", "CPU-clock deltas"),
            CpuObservationSource::SampledOccupancy => (
                "Estimated CPU occupancy",
                "CPU-time samples attributed to endpoint CPUs",
            ),
            CpuObservationSource::LegacyUnknown => {
                ("CPU activity (legacy)", "legacy CPU-clock semantics")
            }
        };
        let header = self.render_cpu_heatmap_header(
            title,
            format!(
                "{} · {} {} · {} buckets · {} per bucket",
                semantics,
                lane_count,
                if heatmap.uses_cpu_lanes {
                    "CPU lanes"
                } else {
                    "thread lanes"
                },
                heatmap.buckets,
                format_duration(heatmap.bucket_duration_ns)
            ),
            !heatmap.uses_cpu_lanes,
            cx,
        );
        let selected_range = self.selected_time_range;
        let heatmap_for_paint = heatmap.clone();
        let heatmap_for_click = heatmap.clone();
        let vertical_scroll = ScrollHandle::new();
        let entity = cx.entity();
        let grid = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_cpu_heatmap(bounds, &heatmap_for_paint, selected_range, window);

                let heatmap = heatmap_for_click.clone();
                let entity = entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
                    if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                        return;
                    }
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if width <= 0.0
                        || height <= 0.0
                        || heatmap.buckets == 0
                        || heatmap.lanes.is_empty()
                    {
                        return;
                    }
                    let x = f32::from(event.position.x - bounds.origin.x).clamp(0.0, width);
                    let bucket = ((x / width) * heatmap.buckets as f32)
                        .floor()
                        .min(heatmap.buckets.saturating_sub(1) as f32)
                        as usize;
                    let start_ns = heatmap
                        .range
                        .start_ns
                        .saturating_add((bucket as u64).saturating_mul(heatmap.bucket_duration_ns));
                    let end_ns = start_ns
                        .saturating_add(heatmap.bucket_duration_ns)
                        .min(heatmap.range.end_ns);
                    if start_ns >= end_ns {
                        return;
                    }

                    entity.update(cx, |view, cx| {
                        view.select_time_range(TimeRange { start_ns, end_ns });
                        cx.notify();
                    });
                });
            },
        )
        .size_full();

        div().size_full().flex().flex_col().child(header).child(
            div()
                .min_h(px(0.0))
                .flex_1()
                .flex()
                .child(
                    div()
                        .w(px(LANE_LABEL_WIDTH))
                        .min_w(px(LANE_LABEL_WIDTH))
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(CHROME))
                        .child(cpu_heatmap_axis_corner(heatmap.uses_cpu_lanes))
                        .child(
                            div()
                                .id("cpu-heatmap-label-scroll")
                                .min_h(px(0.0))
                                .flex_1()
                                .overflow_y_scroll()
                                .track_scroll(&vertical_scroll)
                                .child(cpu_lane_labels(
                                    &heatmap,
                                    effective_lane_height,
                                    grid_height,
                                )),
                        ),
                )
                .child(
                    div()
                        .id("cpu-heatmap-horizontal-scroll")
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .flex_1()
                        .overflow_x_scroll()
                        .bg(rgb(WORKSPACE))
                        .child(
                            div()
                                .w(px(grid_width))
                                .min_w(px(grid_width))
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(cpu_heatmap_axis(&heatmap, grid_width))
                                .child(
                                    div()
                                        .id("cpu-heatmap-scroll")
                                        .min_h(px(0.0))
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .track_scroll(&vertical_scroll)
                                        .child(
                                            div()
                                                .w(px(grid_width))
                                                .min_w(px(grid_width))
                                                .h(px(grid_height))
                                                .child(grid),
                                        ),
                                ),
                        ),
                ),
        )
    }

    fn render_cpu_heatmap_header(
        &self,
        title: &'static str,
        status: String,
        compatibility_mode: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut controls = Vec::with_capacity(BUCKET_PRESETS.len() + 1);
        controls.push(cpu_bucket_button(
            "Auto",
            None,
            self.cpu_heatmap_bucket_ns.is_none(),
            cx,
        ));
        controls.extend(BUCKET_PRESETS.into_iter().map(|(label, duration)| {
            cpu_bucket_button(
                label,
                Some(duration),
                self.cpu_heatmap_bucket_ns == Some(duration),
                cx,
            )
        }));

        div()
            .min_h(px(58.0))
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .h(px(30.0))
                    .min_h(px(30.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(360.0))
                            .truncate()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(status),
                    )
                    .when(compatibility_mode, |element| {
                        element.child(
                            div()
                                .min_w_0()
                                .max_w(px(310.0))
                                .truncate()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .child("Thread compatibility mode · CPU IDs unavailable"),
                        )
                    })
                    .child(div().flex_1())
                    .when(
                        self.selected_time_range.is_some() || self.hotspot_filter.is_some(),
                        |element| {
                            element.child(
                                div()
                                    .id("cpu-heatmap-clear-filter")
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(ACCENT))
                                    .hover(|element| element.bg(rgb(HOVER)))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.clear_global_filter();
                                        cx.notify();
                                    }))
                                    .child("Clear Filters"),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .h(px(28.0))
                    .min_h(px(28.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(div().flex_none().child("Bucket width"))
                    .child(
                        div()
                            .id("cpu-heatmap-bucket-controls")
                            .min_w_0()
                            .flex_1()
                            .overflow_x_scroll()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .whitespace_nowrap()
                                    .children(controls),
                            ),
                    )
                    .child(cpu_utilization_legend()),
            )
    }

    fn render_cpu_heatmap_message(
        &self,
        message: impl Into<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_cpu_heatmap_header(
                "CPU utilization",
                "Quantitative CPU-clock data".to_owned(),
                false,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
                    .child(message.into()),
            )
    }
}

fn cpu_bucket_button(
    label: &'static str,
    duration_ns: Option<u64>,
    selected: bool,
    cx: &mut Context<MperfGui>,
) -> impl IntoElement + use<> {
    div()
        .id(SharedString::from(format!(
            "cpu-bucket-{}",
            label.to_ascii_lowercase().replace(' ', "-")
        )))
        .h(px(20.0))
        .flex()
        .items_center()
        .px_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .when(selected, |element| {
            element
                .bg(rgb(ACCENT))
                .text_color(rgb(0x111111))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |element| {
            element
                .text_color(rgb(MUTED_TEXT))
                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
        })
        .on_click(cx.listener(move |view, _, _, cx| {
            view.cpu_heatmap_bucket_ns = duration_ns;
            cx.notify();
        }))
        .child(label)
}

fn cpu_heatmap_axis(heatmap: &CpuUtilizationHeatmap, grid_width: f32) -> Div {
    let duration = heatmap.range.end_ns.saturating_sub(heatmap.range.start_ns);
    div()
        .w(px(grid_width))
        .min_w(px(grid_width))
        .h(px(26.0))
        .min_h(px(26.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .justify_between()
        .px_1()
        .child("+0")
        .child(format!("+{}", format_duration(duration / 4)))
        .child(format!("+{}", format_duration(duration / 2)))
        .child(format!(
            "+{}",
            format_duration(duration.saturating_mul(3) / 4)
        ))
        .child(format!("+{}", format_duration(duration)))
}

fn cpu_heatmap_axis_corner(uses_cpu_lanes: bool) -> Div {
    div()
        .h(px(26.0))
        .min_h(px(26.0))
        .w_full()
        .flex()
        .items_center()
        .px_2()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .child(if uses_cpu_lanes {
            "CPU"
        } else {
            "THREAD (COMPAT)"
        })
}

fn cpu_lane_labels(heatmap: &CpuUtilizationHeatmap, lane_height: f32, grid_height: f32) -> Div {
    div()
        .w_full()
        .h(px(grid_height))
        .flex()
        .flex_col()
        .bg(rgb(CHROME))
        .children(heatmap.lanes.iter().map(|lane| {
            div()
                .h(px(lane_height))
                .min_h(px(lane_height))
                .flex()
                .items_center()
                .justify_end()
                .pr_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .text_color(rgb(TEXT))
                .child(lane.label.clone())
        }))
}

fn cpu_utilization_legend() -> Div {
    div()
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .pl_3()
        .pr_2()
        .border_l_1()
        .border_color(rgb(BORDER))
        .children([0.0_f64, 0.25, 0.5, 0.75, 1.0].into_iter().map(|value| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .w(px(12.0))
                        .h(px(12.0))
                        .rounded_sm()
                        .bg(rgb(utilization_color(value))),
                )
                .child(format!("{:.0}%", value * 100.0))
        }))
}

fn cpu_grid_width(buckets: usize) -> f32 {
    (buckets as f32 * CELL_TARGET_WIDTH).clamp(MIN_GRID_WIDTH, MAX_GRID_WIDTH)
}

fn paint_cpu_heatmap(
    bounds: Bounds<gpui::Pixels>,
    heatmap: &CpuUtilizationHeatmap,
    selected_range: Option<TimeRange>,
    window: &mut gpui::Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 || heatmap.buckets == 0 || heatmap.lanes.is_empty() {
        return;
    }

    let cell_width = width / heatmap.buckets as f32;
    let cell_height = height / heatmap.lanes.len() as f32;
    for (lane_index, lane) in heatmap.lanes.iter().enumerate() {
        for (bucket, value) in lane.buckets.iter().enumerate() {
            let origin = point(
                bounds.origin.x + px(bucket as f32 * cell_width),
                bounds.origin.y + px(lane_index as f32 * cell_height),
            );
            let cell = Bounds::new(
                origin,
                size(
                    px((cell_width - 1.0).max(0.5)),
                    px((cell_height - 1.0).max(1.0)),
                ),
            );
            let start_ns = heatmap
                .range
                .start_ns
                .saturating_add((bucket as u64).saturating_mul(heatmap.bucket_duration_ns));
            let end_ns = start_ns
                .saturating_add(heatmap.bucket_duration_ns)
                .min(heatmap.range.end_ns);
            let selected = selected_range
                .is_some_and(|selected| selected.start_ns < end_ns && start_ns < selected.end_ns);
            if selected {
                window.paint_quad(fill(cell, rgb(ACCENT)));
                window.paint_quad(fill(
                    cell.inset(px(2.0)),
                    rgb(utilization_color(value.utilization)),
                ));
            } else {
                window.paint_quad(fill(cell, rgb(utilization_color(value.utilization))));
            }
        }
    }
}

fn utilization_color(utilization: f64) -> u32 {
    const ZERO: u32 = 0x18191c;
    const FULL: u32 = 0xe5484d;
    mix_rgb(ZERO, FULL, utilization.clamp(0.0, 1.0) as f32)
}

fn mix_rgb(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let from = ((from >> shift) & 0xff_u32) as f32;
        let to = ((to >> shift) & 0xff_u32) as f32;
        (from + (to - from) * amount).round().clamp(0.0, 255.0) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn format_duration(nanoseconds: u64) -> String {
    if nanoseconds >= 1_000_000_000 {
        let seconds = nanoseconds as f64 / 1_000_000_000.0;
        if seconds >= 10.0 {
            format!("{seconds:.0}s")
        } else {
            format!("{seconds:.2}s")
        }
    } else if nanoseconds >= 1_000_000 {
        format!("{:.1}ms", nanoseconds as f64 / 1_000_000.0)
    } else if nanoseconds >= 1_000 {
        format!("{:.1}µs", nanoseconds as f64 / 1_000.0)
    } else {
        format!("{nanoseconds}ns")
    }
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return u64::MAX;
    }
    value / divisor + u64::from(!value.is_multiple_of(divisor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_width_preserves_detail_without_exceeding_canvas_bound() {
        assert_eq!(cpu_grid_width(1), MIN_GRID_WIDTH);
        assert_eq!(cpu_grid_width(100), 1_100.0);
        assert_eq!(cpu_grid_width(100_000), MAX_GRID_WIDTH);
    }
}
