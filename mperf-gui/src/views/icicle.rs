use gpui::{Context, Div, FontWeight, SharedString, div, prelude::*, px, relative, rgb};

use crate::{
    MperfGui,
    profile::TimeRange,
    profile_analysis::CallTree,
    theme::{
        ACCENT, BORDER, ERROR, FLAME_FRAME_HEIGHT, HOVER, MUTED_TEXT, SELECTION, SURFACE, TEXT,
        WORKSPACE, flame_frame_color,
    },
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

        let filter = self.profile_sample_filter();
        let tree = CallTree::build(profile, &filter);
        if tree.total_samples == 0 {
            return self.render_empty_icicle(cx);
        }

        let requested_focus = find_focus_node(&tree, &self.icicle_focus_path);
        let layout = tree.icicle_layout(requested_focus);
        let focus = &tree.nodes[layout.focus_node];
        let graph_height = ((layout.max_depth + 1) as f32 * FLAME_FRAME_HEIGHT).max(180.0);
        let status = format!(
            "{} · {} samples in view · {} inclusive / {} self",
            focus.label, layout.total_samples, focus.inclusive_samples, focus.self_samples
        );
        let header = self.render_icicle_header(
            status,
            layout.focus_path.len() > 1,
            self.selected_time_range,
            profile.full_range(),
            cx,
        );
        let total_samples = layout.total_samples;

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("icicle-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_scroll()
                .p_1()
                .child(
                    div()
                        .relative()
                        .min_w(px(920.0))
                        .w_full()
                        .h(px(graph_height))
                        .children(layout.frames.into_iter().map(|frame| {
                            let color = flame_frame_color(&frame.label, frame.depth);
                            let focus_path = tree
                                .focus_path(frame.node_id)
                                .into_iter()
                                .filter_map(|node_id| tree.nodes[node_id].frame_id)
                                .collect::<Vec<_>>();
                            let frame_id = frame.frame_id;
                            let selected = frame.node_id == layout.focus_node;
                            let label = if frame.width > 0.08 {
                                format!(
                                    "{} · {} incl / {} self · {:.1}%",
                                    frame.label,
                                    frame.inclusive_samples,
                                    frame.self_samples,
                                    frame.inclusive_samples as f64 / total_samples as f64 * 100.0
                                )
                            } else if frame.width > 0.025 {
                                frame.label.clone()
                            } else {
                                String::new()
                            };

                            div()
                                .id(SharedString::from(format!(
                                    "icicle-frame-{}-{}",
                                    frame.node_id,
                                    frame
                                        .frame_id
                                        .map_or_else(|| "root".to_string(), |id| id.to_string())
                                )))
                                .absolute()
                                .left(relative(frame.x as f32))
                                .top(px(frame.depth as f32 * FLAME_FRAME_HEIGHT))
                                .w(relative((frame.width as f32).max(0.000_5)))
                                .h(px(FLAME_FRAME_HEIGHT - 1.0))
                                .flex()
                                .items_center()
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(if selected { ACCENT } else { WORKSPACE }))
                                .bg(rgb(color))
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_xs()
                                .text_color(rgb(0xf7fbff))
                                .when(selected, |element| {
                                    element
                                        .border_2()
                                        .border_color(rgb(SELECTION))
                                        .font_weight(FontWeight::SEMIBOLD)
                                })
                                .hover(|element| element.border_2().border_color(rgb(TEXT)))
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    if let Some(frame_id) = frame_id {
                                        view.select_profile_function(frame_id);
                                        view.icicle_focus_path.clone_from(&focus_path);
                                    } else {
                                        view.icicle_focus_path.clear();
                                    }
                                    cx.notify();
                                }))
                                .child(label)
                        })),
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

fn find_focus_node(tree: &CallTree, frame_path: &[usize]) -> Option<usize> {
    if frame_path.is_empty() {
        return None;
    }
    tree.nodes.iter().find_map(|node| {
        let candidate = tree
            .focus_path(node.id)
            .into_iter()
            .filter_map(|node_id| tree.nodes[node_id].frame_id)
            .collect::<Vec<_>>();
        (candidate == frame_path).then_some(node.id)
    })
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
