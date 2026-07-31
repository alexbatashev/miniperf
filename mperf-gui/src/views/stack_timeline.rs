use gpui::{
    div, prelude::*, px, relative, rgb, Context, Div, FontWeight, IntoElement, ScrollHandle,
    SharedString,
};

use crate::{
    profile::TimeRange,
    profile_analysis::{TimeOrderLane, TimeOrderTimeline},
    theme::{
        flame_frame_color, ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION, SURFACE,
        TEXT, WORKSPACE,
    },
    MperfGui,
};

const MAX_BINS: usize = 240;
const LANE_LABEL_WIDTH: f32 = 118.0;
const MIN_TIMELINE_WIDTH: f32 = 1040.0;
const FRAME_HEIGHT: f32 = 16.0;
const LANE_PADDING: f32 = 2.0;

impl MperfGui {
    pub(crate) fn render_stack_timeline_workspace(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(model) = self.model.as_ref() else {
            return timeline_message("Open a recording to inspect stacks over time.", false);
        };
        let profile = &model.profile;
        if let Some(error) = profile.error.as_ref() {
            return timeline_message(error.clone(), true);
        }
        if profile.samples.is_empty() {
            return timeline_message(
                "This recording has no timestamped stack samples for a timeline.",
                false,
            );
        }

        let filter = self.profile_sample_filter();
        let selected_frames = filter.frame_ids.clone().unwrap_or_default();
        let Some(timeline) = TimeOrderTimeline::build(profile, &filter, MAX_BINS) else {
            return timeline_message(
                "The matching samples do not span a measurable time range.",
                false,
            );
        };
        if timeline.total_samples == 0 || timeline.lanes.is_empty() {
            return timeline_message("No stack samples match the active filters.", false);
        }

        let recording_start = profile
            .full_range()
            .map(|range| range.start_ns)
            .unwrap_or(timeline.range.start_ns);
        let header = self.render_stack_timeline_header(&timeline, cx);
        let range_duration = timeline
            .range
            .end_ns
            .saturating_sub(timeline.range.start_ns)
            .max(1);
        let bin_width_ns = timeline.bin_width_ns;
        let vertical_scroll = ScrollHandle::new();

        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(header)
            .child(
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
                            .child(render_time_axis_corner(timeline.uses_cpu_lanes))
                            .child(
                                div()
                                    .id("stack-timeline-label-scroll")
                                    .min_h(px(0.0))
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .track_scroll(&vertical_scroll)
                                    .children(timeline.lanes.iter().map(|lane| {
                                        render_stack_timeline_lane_label(
                                            lane,
                                            stack_timeline_lane_height(lane),
                                        )
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("stack-timeline-horizontal-scroll")
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .flex_1()
                            .overflow_x_scroll()
                            .bg(rgb(WORKSPACE))
                            .child(
                                div()
                                    .w(px(MIN_TIMELINE_WIDTH))
                                    .min_w(px(MIN_TIMELINE_WIDTH))
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(render_time_axis(&timeline, recording_start))
                                    .child(
                                        div()
                                            .id("stack-timeline-scroll")
                                            .min_h(px(0.0))
                                            .flex_1()
                                            .overflow_y_scroll()
                                            .track_scroll(&vertical_scroll)
                                            .children(timeline.lanes.iter().enumerate().map(
                                                |(lane_index, lane)| {
                                                    self.render_stack_timeline_lane_plot(
                                                        lane_index,
                                                        lane,
                                                        timeline.range,
                                                        range_duration,
                                                        bin_width_ns,
                                                        &selected_frames,
                                                        cx,
                                                    )
                                                },
                                            )),
                                    ),
                            ),
                    ),
            )
    }

    fn render_stack_timeline_header(
        &self,
        timeline: &TimeOrderTimeline,
        cx: &mut Context<Self>,
    ) -> Div {
        let lane_kind = if timeline.uses_cpu_lanes {
            "CPU stack lanes"
        } else {
            "Thread compatibility lanes"
        };
        let status = format!(
            "{} samples · {} lanes · {} bins · {} per bin",
            timeline.total_samples,
            timeline.lanes.len(),
            timeline.bins,
            format_duration(timeline.bin_width_ns),
        );
        let has_active_filter = self.selected_time_range.is_some() || self.hotspot_filter.is_some();

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
            .child(div().font_weight(FontWeight::SEMIBOLD).child(lane_kind))
            .when(!timeline.uses_cpu_lanes, |element| {
                element.child(
                    div()
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .px_2()
                        .rounded_sm()
                        .bg(rgb(CHROME))
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .child("CPU IDs unavailable"),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(status),
            )
            .when(has_active_filter, |element| {
                element.child(
                    div()
                        .id("stack-timeline-clear-filters")
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.clear_global_filter();
                            cx.notify();
                        }))
                        .child("Clear Filters"),
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_stack_timeline_lane_plot(
        &self,
        lane_index: usize,
        lane: &TimeOrderLane,
        range: TimeRange,
        range_duration: u64,
        bin_width_ns: u64,
        selected_frames: &std::collections::BTreeSet<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let lane_height = stack_timeline_lane_height(lane);

        div()
            .h(px(lane_height))
            .min_h(px(lane_height))
            .w_full()
            .relative()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(WORKSPACE))
            .children(
                lane.segments
                    .iter()
                    .enumerate()
                    .map(|(segment_index, segment)| {
                        let start = segment.start_ns.saturating_sub(range.start_ns);
                        let end = segment.end_ns.saturating_sub(range.start_ns);
                        let x = start as f64 / range_duration as f64;
                        let width = end.saturating_sub(start) as f64 / range_duration as f64;
                        let frame_id = segment.frame_id;
                        let segment_duration = segment.end_ns.saturating_sub(segment.start_ns);
                        let selection_start = segment
                            .start_ns
                            .saturating_add(segment_duration.saturating_sub(bin_width_ns) / 2);
                        let selection_end = selection_start
                            .saturating_add(bin_width_ns)
                            .min(segment.end_ns);
                        let selected = selected_frames.contains(&frame_id);
                        let label = if width > 0.075 {
                            format!(
                                "{} · {}/{} samples",
                                segment.label, segment.representative_samples, segment.samples
                            )
                        } else if width > 0.025 {
                            segment.label.clone()
                        } else {
                            String::new()
                        };
                        let color = flame_frame_color(&segment.label, frame_id);

                        div()
                            .id(SharedString::from(format!(
                                "stack-timeline-{lane_index}-{segment_index}"
                            )))
                            .absolute()
                            .left(relative(x as f32))
                            .top(px(LANE_PADDING + segment.depth as f32 * FRAME_HEIGHT))
                            .w(relative((width as f32).max(0.000_5)))
                            .h(px(FRAME_HEIGHT - 1.0))
                            .flex()
                            .items_center()
                            .px_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(if selected { SELECTION } else { WORKSPACE }))
                            .bg(rgb(color))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(0xf7fbff))
                            .when(selected, |element| {
                                element
                                    .border_2()
                                    .border_color(rgb(ACCENT))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(|element| element.border_2().border_color(rgb(TEXT)))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if selection_start < selection_end {
                                    view.select_time_range(TimeRange {
                                        start_ns: selection_start,
                                        end_ns: selection_end,
                                    });
                                }
                                view.select_profile_function(frame_id);
                                cx.notify();
                            }))
                            .child(label)
                    }),
            )
    }
}

fn render_time_axis(timeline: &TimeOrderTimeline, recording_start: u64) -> Div {
    let duration = timeline
        .range
        .end_ns
        .saturating_sub(timeline.range.start_ns);
    let start_offset = timeline.range.start_ns.saturating_sub(recording_start);

    div()
        .w(px(MIN_TIMELINE_WIDTH))
        .min_w(px(MIN_TIMELINE_WIDTH))
        .h(px(30.0))
        .min_h(px(30.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .justify_between()
        .px_1()
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .children((0..=4).map(|tick| {
            let offset = start_offset.saturating_add(duration.saturating_mul(tick) / 4);
            div().child(format!("+{}", format_duration(offset)))
        }))
}

fn render_time_axis_corner(uses_cpu_lanes: bool) -> Div {
    div()
        .h(px(30.0))
        .min_h(px(30.0))
        .w_full()
        .flex()
        .items_center()
        .px_2()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .child(if uses_cpu_lanes {
            "CPU / STACK"
        } else {
            "THREAD / STACK"
        })
}

fn render_stack_timeline_lane_label(lane: &TimeOrderLane, lane_height: f32) -> Div {
    div()
        .h(px(lane_height))
        .min_h(px(lane_height))
        .w_full()
        .flex()
        .flex_col()
        .justify_center()
        .px_2()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .child(
            div()
                .truncate()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(lane.label.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(format!("{} stack spans", lane.segments.len())),
        )
}

fn stack_timeline_lane_height(lane: &TimeOrderLane) -> f32 {
    let lane_depth = lane
        .segments
        .iter()
        .map(|segment| segment.depth + 1)
        .max()
        .unwrap_or(1);
    lane_depth as f32 * FRAME_HEIGHT + LANE_PADDING * 2.0
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

fn timeline_message(message: impl Into<String>, error: bool) -> Div {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_analysis::{TimeOrderSegment, TimelineLaneKey};

    #[test]
    fn lane_height_tracks_the_deepest_stack_frame() {
        let empty_lane = TimeOrderLane {
            key: TimelineLaneKey::Cpu(Some(0)),
            label: "CPU 0".to_owned(),
            segments: Vec::new(),
        };
        assert_eq!(
            stack_timeline_lane_height(&empty_lane),
            FRAME_HEIGHT + LANE_PADDING * 2.0
        );

        let lane = TimeOrderLane {
            segments: vec![TimeOrderSegment {
                depth: 4,
                frame_id: 7,
                label: "work".to_owned(),
                path: vec![1, 2, 3, 4, 7],
                start_ns: 10,
                end_ns: 20,
                samples: 3,
                representative_samples: 2,
            }],
            ..empty_lane
        };
        assert_eq!(
            stack_timeline_lane_height(&lane),
            FRAME_HEIGHT * 5.0 + LANE_PADDING * 2.0
        );
    }
}
