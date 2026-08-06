use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};

use crate::{
    MperfGui,
    profile::TimeRange,
    profile_analysis::{CounterAggregation, CounterTrack, CounterTracks},
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED, SURFACE, TEXT, WORKSPACE,
    },
};

const MAX_BINS: usize = 160;
const TRACK_HEIGHT: f32 = 70.0;
const LABEL_WIDTH: f32 = 188.0;
const MIN_GRAPH_WIDTH: f32 = 760.0;

impl MperfGui {
    pub(crate) fn render_metrics_timeline_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return metrics_message("Open a recording to inspect counter tracks.", false);
        };
        if let Some(error) = model.profile.error.clone() {
            return metrics_message(error, true);
        }

        let filter = self.profile_sample_filter();
        let Some(tracks) = CounterTracks::build(&model.profile, &filter, MAX_BINS) else {
            return metrics_message(
                "This recording has no timestamped counter samples for a metrics timeline.",
                false,
            );
        };
        if tracks.sample_counts.iter().all(|samples| *samples == 0) {
            return self.render_empty_metrics_timeline(cx);
        }

        let sample_total = tracks.sample_counts.iter().sum::<u64>();
        let status = format!(
            "{} samples · {} tracks · {} bins · {} per bin",
            sample_total,
            tracks.tracks.len() + 1,
            tracks.bins,
            format_duration(tracks.bin_width_ns),
        );
        let selected_range = self.selected_time_range;
        let mut rows = Vec::with_capacity(tracks.tracks.len() + 1);
        rows.push(self.render_sample_count_track(&tracks, selected_range, cx));
        rows.extend(tracks.tracks.iter().enumerate().map(|(index, track)| {
            self.render_counter_track(index, &tracks, track, selected_range, cx)
        }));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(
                div()
                    .h(px(30.0))
                    .min_h(px(30.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Time-aligned counters"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(status),
                    )
                    .when_some(self.selected_function.clone(), |element, function| {
                        element.child(
                            div()
                                .max_w(px(280.0))
                                .truncate()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .child(format!("Function: {function}")),
                        )
                    })
                    .when(
                        self.selected_time_range.is_some() || self.hotspot_filter.is_some(),
                        |element| {
                            element.child(
                                div()
                                    .id("metrics-timeline-clear-filter")
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
                                    .child("Clear Filter"),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .h(px(24.0))
                    .min_h(px(24.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(
                        div()
                            .w(px(LABEL_WIDTH))
                            .min_w(px(LABEL_WIDTH))
                            .px_2()
                            .child("TRACK"),
                    )
                    .child(
                        div()
                            .min_w(px(MIN_GRAPH_WIDTH))
                            .flex_1()
                            .flex()
                            .justify_between()
                            .px_1()
                            .child("START")
                            .child(format!(
                                "+{}",
                                format_duration(
                                    tracks.range.end_ns.saturating_sub(tracks.range.start_ns)
                                )
                            )),
                    ),
            )
            .child(
                div()
                    .id("metrics-timeline-scroll")
                    .min_h(px(0.0))
                    .flex_1()
                    .overflow_scroll()
                    .child(
                        div()
                            .min_w(px(LABEL_WIDTH + MIN_GRAPH_WIDTH))
                            .children(rows),
                    ),
            )
    }

    fn render_sample_count_track(
        &self,
        tracks: &CounterTracks,
        selected_range: Option<TimeRange>,
        cx: &mut Context<Self>,
    ) -> Div {
        let values = tracks
            .sample_counts
            .iter()
            .map(|value| Some(*value as f64))
            .collect::<Vec<_>>();
        let total = tracks.sample_counts.iter().sum::<u64>();
        self.render_track_row(
            "Sample observations".to_string(),
            format!("{total} total · count per time bin"),
            values,
            tracks,
            selected_range,
            ACCENT,
            cx,
        )
    }

    fn render_counter_track(
        &self,
        index: usize,
        tracks: &CounterTracks,
        track: &CounterTrack,
        selected_range: Option<TimeRange>,
        cx: &mut Context<Self>,
    ) -> Div {
        let aggregation = match track.aggregation {
            CounterAggregation::Sum => "sum per time bin",
            CounterAggregation::Ratio => "ratio of per-bin sums",
        };
        let maximum = track
            .values
            .iter()
            .flatten()
            .copied()
            .filter(|value| value.is_finite())
            .fold(0.0_f64, f64::max);
        let detail = if maximum > 0.0 {
            format!("{aggregation} · max {}", format_metric_value(maximum))
        } else {
            format!("{aggregation} · no values")
        };
        let color = metric_color(index);
        self.render_track_row(
            track.label.clone(),
            detail,
            track.values.clone(),
            tracks,
            selected_range,
            color,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_track_row(
        &self,
        label: String,
        detail: String,
        values: Vec<Option<f64>>,
        tracks: &CounterTracks,
        selected_range: Option<TimeRange>,
        color: u32,
        cx: &mut Context<Self>,
    ) -> Div {
        let range = tracks.range;
        let bin_width_ns = tracks.bin_width_ns;
        let bins = tracks.bins;
        let paint_values = values;
        let entity = cx.entity();
        let graph = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_track(bounds, &paint_values, color, selected_range, range, window);
                let entity = entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
                    if event.button != MouseButton::Left
                        || bins == 0
                        || !bounds.contains(&event.position)
                    {
                        return;
                    }
                    let width = f32::from(bounds.size.width);
                    if width <= 0.0 {
                        return;
                    }
                    let x = f32::from(event.position.x - bounds.origin.x).clamp(0.0, width);
                    let bin = ((x / width) * bins as f32)
                        .floor()
                        .min(bins.saturating_sub(1) as f32) as usize;
                    let start_ns = range
                        .start_ns
                        .saturating_add((bin as u64).saturating_mul(bin_width_ns));
                    let end_ns = start_ns.saturating_add(bin_width_ns).min(range.end_ns);
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

        div()
            .h(px(TRACK_HEIGHT))
            .min_h(px(TRACK_HEIGHT))
            .flex()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .min_w(px(LABEL_WIDTH))
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_1()
                    .px_3()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(detail),
                    ),
            )
            .child(
                div()
                    .min_w(px(MIN_GRAPH_WIDTH))
                    .h_full()
                    .flex_1()
                    .p_2()
                    .cursor_pointer()
                    .child(graph),
            )
    }

    fn render_empty_metrics_timeline(&self, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(30.0))
                    .min_h(px(30.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Time-aligned counters")
                    .child(div().flex_1())
                    .when(
                        self.selected_time_range.is_some() || self.hotspot_filter.is_some(),
                        |element| {
                            element.child(
                                div()
                                    .id("metrics-empty-clear-filter")
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
                                    .child("Clear Filter"),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(MUTED_TEXT))
                    .child("No counter samples match the active filter."),
            )
    }
}

fn paint_track(
    bounds: Bounds<gpui::Pixels>,
    values: &[Option<f64>],
    color: u32,
    selected_range: Option<TimeRange>,
    range: TimeRange,
    window: &mut gpui::Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 || values.is_empty() {
        return;
    }
    window.paint_quad(fill(bounds, rgb(SURFACE)));

    let maximum = values
        .iter()
        .flatten()
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .fold(0.0_f64, f64::max);
    let bar_width = width / values.len() as f32;
    for (index, value) in values.iter().enumerate() {
        let left = index as f32 * bar_width;
        let cell = Bounds::new(
            point(bounds.origin.x + px(left), bounds.origin.y),
            size(px(bar_width.max(1.0)), bounds.size.height),
        );
        if selected_range.is_some_and(|selected| {
            let duration = range.end_ns.saturating_sub(range.start_ns);
            let start_ns = range
                .start_ns
                .saturating_add(((index as u128 * duration as u128) / values.len() as u128) as u64);
            let end_ns = range.start_ns.saturating_add(
                (((index + 1) as u128 * duration as u128) / values.len() as u128) as u64,
            );
            selected.start_ns < end_ns && start_ns < selected.end_ns
        }) {
            window.paint_quad(fill(cell, rgb(SELECTION_MUTED)));
        }
        let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0) else {
            continue;
        };
        let fraction = if maximum > 0.0 {
            (value / maximum).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let bar_height = (height * fraction).max(1.0);
        let bar = Bounds::new(
            point(
                bounds.origin.x + px(left + 1.0),
                bounds.origin.y + px(height - bar_height),
            ),
            size(px((bar_width - 2.0).max(1.0)), px(bar_height)),
        );
        window.paint_quad(fill(bar, rgb(color)));
    }
}

fn metric_color(index: usize) -> u32 {
    const COLORS: [u32; 8] = [
        0xd2a45f, 0xc66a61, 0x9f7bb3, 0x6f9ca5, 0x8a9b62, 0xc38856, 0x7b83ad, 0xb06f83,
    ];
    COLORS[index % COLORS.len()]
}

fn format_metric_value(value: f64) -> String {
    if value.abs() >= 1_000_000_000.0 {
        format!("{:.2}G", value / 1_000_000_000.0)
    } else if value.abs() >= 1_000_000.0 {
        format!("{:.2}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.2}K", value / 1_000.0)
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.3}")
    }
}

fn format_duration(nanoseconds: u64) -> String {
    if nanoseconds >= 1_000_000_000 {
        format!("{:.3}s", nanoseconds as f64 / 1_000_000_000.0)
    } else if nanoseconds >= 1_000_000 {
        format!("{:.1}ms", nanoseconds as f64 / 1_000_000.0)
    } else if nanoseconds >= 1_000 {
        format!("{:.1}µs", nanoseconds as f64 / 1_000.0)
    } else {
        format!("{nanoseconds}ns")
    }
}

fn metrics_message(message: impl Into<String>, error: bool) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
        .child(message.into())
}
