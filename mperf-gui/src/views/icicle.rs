use std::rc::Rc;

use gpui::{Context, Div, FontWeight, div, prelude::*, px, rgb};

use crate::{
    MperfGui,
    profile::TimeRange,
    theme::{ACCENT, BORDER, ERROR, HOVER, MUTED_TEXT, SURFACE, TEXT},
    views::flame_canvas::{LabelStyle, flame_chart_canvas},
};

impl MperfGui {
    pub(crate) fn render_icicle_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return icicle_message("Open a recording to inspect its call tree.", false);
        };
        let profile = &model.profile;
        if let Some(error) = profile.error.clone() {
            return icicle_message(error, true);
        }
        let full_range = profile.full_range();

        let (Some(tree), Some(chart)) = (self.cached_call_tree(), self.cached_icicle_chart())
        else {
            return icicle_message("Open a recording to inspect its call tree.", false);
        };
        if chart.total == 0 {
            return self.render_empty_icicle(cx);
        }

        let focus = chart.frames.iter().find(|frame| frame.depth == 0);
        let status = self
            .hovered_frame
            .as_ref()
            .map(|frame| {
                format!(
                    "{} · {} inclusive / {} self",
                    frame.name, frame.value, frame.self_value
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{} · {} samples in view · {} inclusive / {} self",
                    chart.root_name,
                    chart.total,
                    focus.map(|frame| frame.total).unwrap_or_default(),
                    focus.map(|frame| frame.self_total).unwrap_or_default()
                )
            });
        let header = self.render_icicle_header(
            status,
            chart.focused,
            self.selected_time_range,
            full_range,
            cx,
        );
        let graph_height = chart.graph_height();
        let graph = flame_chart_canvas(
            chart,
            self.hovered_frame.as_ref().map(|frame| frame.id),
            None,
            None,
            LabelStyle::InclSelf,
            cx.entity(),
            None,
            Rc::new(move |view, frame, cx| {
                if let Some(frame_id) = frame.frame_id {
                    let focus_path = tree
                        .focus_path(frame.key)
                        .into_iter()
                        .filter_map(|node_id| tree.nodes[node_id].frame_id)
                        .collect::<Vec<_>>();
                    view.select_profile_function(frame_id);
                    view.icicle_focus_path = focus_path;
                } else {
                    view.icicle_focus_path.clear();
                }
                cx.notify();
            }),
        );

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("icicle-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_scroll()
                .p_1()
                .child(
                    div()
                        .min_w(px(920.0))
                        .w_full()
                        .h(px(graph_height))
                        .child(graph.size_full()),
                ),
        )
    }

    fn render_icicle_header(
        &self,
        status: String,
        focused: bool,
        selected_range: Option<TimeRange>,
        full_range: Option<TimeRange>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .h(px(28.0))
            .min_h(px(28.0))
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
                    .child("Top-down call tree"),
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
            .when_some(selected_range, |element, range| {
                element.child(
                    div()
                        .max_w(px(280.0))
                        .truncate()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(format!(
                            "Range {}",
                            full_range
                                .map(|full| format_range(range, full.start_ns))
                                .unwrap_or_else(|| format_duration(
                                    range.end_ns.saturating_sub(range.start_ns)
                                ))
                        )),
                )
            })
            .when(focused, |element| {
                element.child(icicle_button(
                    "icicle-reset-focus",
                    "Reset Focus",
                    cx.listener(|view, _, _, cx| {
                        view.icicle_focus_path.clear();
                        cx.notify();
                    }),
                ))
            })
            .when(selected_range.is_some(), |element| {
                element.child(icicle_button(
                    "icicle-clear-filter",
                    "Clear Filters",
                    cx.listener(|view, _, _, cx| {
                        view.clear_global_filter();
                        cx.notify();
                    }),
                ))
            })
            .when(
                selected_range.is_none() && self.hotspot_filter.is_some(),
                |element| {
                    element.child(icicle_button(
                        "icicle-clear-function-filter",
                        "Clear Filter",
                        cx.listener(|view, _, _, cx| {
                            view.clear_global_filter();
                            cx.notify();
                        }),
                    ))
                },
            )
    }

    fn render_empty_icicle(&self, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.0))
                    .min_h(px(28.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Top-down call tree")
                    .child(div().flex_1())
                    .when(
                        self.selected_time_range.is_some() || self.hotspot_filter.is_some(),
                        |element| {
                            element.child(icicle_button(
                                "icicle-empty-clear-filter",
                                "Clear Filters",
                                cx.listener(|view, _, _, cx| {
                                    view.clear_global_filter();
                                    cx.notify();
                                }),
                            ))
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
                    .child(
                        if self.selected_time_range.is_some() || self.hotspot_filter.is_some() {
                            "No call stacks match the active filters."
                        } else {
                            "This recording has no call stacks for an icicle graph."
                        },
                    ),
            )
    }
}

fn icicle_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(20.0))
        .flex()
        .items_center()
        .px_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .text_color(rgb(ACCENT))
        .hover(|element| element.bg(rgb(HOVER)))
        .on_click(on_click)
        .child(label)
}

fn format_range(range: TimeRange, origin_ns: u64) -> String {
    let start = range.start_ns.saturating_sub(origin_ns);
    let end = range.end_ns.saturating_sub(origin_ns);
    format!(
        "{}–{} ({})",
        format_duration(start),
        format_duration(end),
        format_duration(range.end_ns.saturating_sub(range.start_ns))
    )
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

fn icicle_message(message: impl Into<String>, error: bool) -> Div {
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
