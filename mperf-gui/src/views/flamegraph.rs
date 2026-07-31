use gpui::{div, prelude::*, px, relative, rgb, Context, Div, FontWeight, SharedString, Window};
use num_format::ToFormattedString;

use crate::{
    flamegraph::FlamegraphData,
    profile_analysis::CallTree,
    theme::{
        flame_frame_color, ACCENT, BORDER, ERROR, FLAME_FRAME_HEIGHT, HOVER, MUTED_TEXT, SELECTION,
        SURFACE, TEXT, TOOLBAR, WORKSPACE,
    },
    HoveredFrame, MperfGui,
};

impl MperfGui {
    pub(crate) fn render_flamegraph_workspace(&self, cx: &mut Context<Self>) -> Div {
        if self.selected_time_range.is_some() || self.hotspot_filter.is_some() {
            return self.render_filtered_flamegraph(cx);
        }
        let Some(flamegraph) = self.flamegraph_data() else {
            return empty_flamegraph("This recording does not include a flamegraph.");
        };
        self.render_flamegraph(flamegraph, cx)
    }

    fn render_filtered_flamegraph(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return empty_flamegraph("Open a recording to inspect its flamegraph.");
        };
        if let Some(error) = model.profile.error.as_ref() {
            return filtered_flamegraph_message(error.clone(), true);
        }

        let tree = CallTree::build(&model.profile, &self.profile_sample_filter());
        if tree.total_samples == 0 {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.render_filtered_flamegraph_header(
                    "No samples match the active filters".to_string(),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_color(rgb(MUTED_TEXT))
                        .child("Clear the time or function filter to restore samples."),
                );
        }

        let layout = tree.icicle_layout(None);
        let total = layout.total_samples;
        let max_depth = layout.max_depth;
        let graph_height = ((max_depth + 1) as f32 * FLAME_FRAME_HEIGHT).max(180.0);
        let status = format!(
            "{} profile samples · primary-symbol stacks · active global filter",
            total.to_formatted_string(&num_format::Locale::en)
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_filtered_flamegraph_header(status, cx))
            .child(
                div()
                    .id("filtered-flamegraph-scroll")
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
                                let top = (max_depth.saturating_sub(frame.depth)) as f32
                                    * FLAME_FRAME_HEIGHT;
                                let color = if frame.frame_id.is_some() {
                                    flame_frame_color(&frame.label, frame.depth)
                                } else {
                                    SURFACE
                                };
                                let selected = frame.frame_id.is_some()
                                    && self.selected_function.as_ref().is_some_and(|function| {
                                        crate::profile_names_match(&frame.label, function)
                                    });
                                let label = if frame.width > 0.025 {
                                    format!(
                                        "{}  {:.1}%",
                                        frame.label,
                                        frame.inclusive_samples as f64 / total as f64 * 100.0
                                    )
                                } else {
                                    String::new()
                                };
                                let frame_id = frame.frame_id;
                                let tooltip_name = frame.label.clone();
                                let tooltip_total = frame.inclusive_samples;
                                let tooltip_self = frame.self_samples;

                                div()
                                    .id(SharedString::from(format!(
                                        "filtered-flame-frame-{}",
                                        frame.node_id
                                    )))
                                    .absolute()
                                    .left(relative(frame.x as f32))
                                    .top(px(top))
                                    .w(relative((frame.width as f32).max(0.000_5)))
                                    .h(px(FLAME_FRAME_HEIGHT - 1.0))
                                    .flex()
                                    .items_center()
                                    .px_1()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(if selected { ACCENT } else { WORKSPACE }))
                                    .bg(rgb(color))
                                    .when(frame_id.is_some(), |element| {
                                        element.cursor_pointer().hover(|element| {
                                            element.border_2().border_color(rgb(TEXT))
                                        })
                                    })
                                    .when(selected, |element| {
                                        element
                                            .border_2()
                                            .border_color(rgb(SELECTION))
                                            .font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(rgb(0xf7fbff))
                                    .when_some(frame_id, |element, frame_id| {
                                        element.on_click(cx.listener(move |view, _, _, cx| {
                                            view.select_profile_function(frame_id);
                                            cx.notify();
                                        }))
                                    })
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| FlameFrameTooltip {
                                            name: tooltip_name.clone(),
                                            total: format!(
                                                "{} profile samples",
                                                tooltip_total
                                                    .to_formatted_string(&num_format::Locale::en)
                                            ),
                                            self_value: format!(
                                                "{} self",
                                                tooltip_self
                                                    .to_formatted_string(&num_format::Locale::en)
                                            ),
                                        })
                                        .into()
                                    })
                                    .child(label)
                            })),
                    ),
            )
    }

    fn render_filtered_flamegraph_header(&self, status: String, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(28.0))
            .min_h(px(28.0))
            .flex()
            .items_center()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(TOOLBAR))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Filtered flamegraph"),
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
            .child(toolbar_button(
                "Clear Filters",
                cx.listener(|view, _, _, cx| {
                    view.clear_global_filter();
                    cx.notify();
                }),
            ))
    }

    fn render_flamegraph(&self, flamegraph: &FlamegraphData, cx: &mut Context<Self>) -> Div {
        let has_cycles = flamegraph.cycles.is_some();
        let has_instructions = flamegraph.instructions.is_some();
        let layout = flamegraph.layout(self.flamegraph_instructions, self.flamegraph_zoom);

        let Some(layout) = layout else {
            let header = self.render_flamegraph_header(
                "Flamegraph unavailable".to_string(),
                false,
                has_cycles,
                has_instructions,
                cx,
            );
            return div().size_full().flex().flex_col().child(header).child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_color(rgb(ERROR))
                    .child(
                        flamegraph
                            .error
                            .clone()
                            .unwrap_or_else(|| "No flamegraph data available".to_string()),
                    ),
            );
        };

        let graph_height = ((layout.max_depth + 1) as f32 * FLAME_FRAME_HEIGHT).max(180.0);
        let metric_name = if self.flamegraph_instructions {
            "instruction samples"
        } else {
            "cycle samples"
        };
        let detail = self.hovered_frame.as_ref().map(|frame| {
            format!(
                "{}  ·  {} {} total  ·  {} self",
                frame.name,
                frame.value.to_formatted_string(&num_format::Locale::en),
                metric_name,
                frame
                    .self_value
                    .to_formatted_string(&num_format::Locale::en)
            )
        });
        let status = detail.unwrap_or_else(|| {
            format!(
                "{}  ·  {} {}  ·  Hover for details, click to focus and filter Hotspots",
                layout.root_name,
                layout.total.to_formatted_string(&num_format::Locale::en),
                metric_name
            )
        });
        let max_depth = layout.max_depth;
        let total = layout.total;
        let header = self.render_flamegraph_header(
            status,
            self.hovered_frame.is_some(),
            has_cycles,
            has_instructions,
            cx,
        );

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("flamegraph-scroll")
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
                            let top =
                                (max_depth.saturating_sub(frame.depth)) as f32 * FLAME_FRAME_HEIGHT;
                            let selected = self.selected_stack == Some(frame.id);
                            let color = flame_frame_color(&frame.name, frame.depth);
                            let label = if frame.width > 0.025 {
                                format!(
                                    "{}  {:.1}%",
                                    frame.name,
                                    frame.value as f64 / total as f64 * 100.0
                                )
                            } else {
                                String::new()
                            };
                            let id = frame.id;
                            let is_synthetic_root = id == flamelens::flame::ROOT_ID;
                            let function = frame.name.clone();
                            let hover_name = frame.name.clone();
                            let hover_value = frame.value;
                            let hover_self_value = frame.self_value;
                            let tooltip_name = frame.name.clone();
                            let tooltip_total = frame.value;
                            let tooltip_self = frame.self_value;
                            let tooltip_metric = metric_name;

                            div()
                                .id(SharedString::from(format!("flame-frame-{id}")))
                                .absolute()
                                .left(relative(frame.x))
                                .top(px(top))
                                .w(relative(frame.width.max(0.000_5)))
                                .h(px(FLAME_FRAME_HEIGHT - 1.0))
                                .flex()
                                .items_center()
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(if selected { ACCENT } else { WORKSPACE }))
                                .bg(rgb(color))
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
                                .when(!is_synthetic_root, |element| {
                                    element
                                        .cursor_pointer()
                                        .hover(|element| element.border_2().border_color(rgb(TEXT)))
                                })
                                .on_hover(cx.listener(move |view, hovered, _, cx| {
                                    if *hovered {
                                        view.hovered_frame = Some(HoveredFrame {
                                            id,
                                            name: hover_name.clone(),
                                            value: hover_value,
                                            self_value: hover_self_value,
                                        });
                                    } else if view
                                        .hovered_frame
                                        .as_ref()
                                        .is_some_and(|frame| frame.id == id)
                                    {
                                        view.hovered_frame = None;
                                    }
                                    cx.notify();
                                }))
                                .when(!is_synthetic_root, |element| {
                                    element
                                        .tooltip(move |_, cx| {
                                            cx.new(|_| FlameFrameTooltip {
                                                name: tooltip_name.clone(),
                                                total: format!(
                                                    "{} {} total",
                                                    tooltip_total.to_formatted_string(
                                                        &num_format::Locale::en
                                                    ),
                                                    tooltip_metric
                                                ),
                                                self_value: format!(
                                                    "{} self",
                                                    tooltip_self.to_formatted_string(
                                                        &num_format::Locale::en
                                                    )
                                                ),
                                            })
                                            .into()
                                        })
                                        .on_click(cx.listener(
                                            move |view, _: &gpui::ClickEvent, _, cx| {
                                                view.select_flame_frame(id, function.clone());
                                                cx.notify();
                                            },
                                        ))
                                })
                                .child(label)
                        })),
                ),
        )
    }

    fn render_flamegraph_header(
        &self,
        status: String,
        emphasized: bool,
        has_cycles: bool,
        has_instructions: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .h(px(28.0))
            .min_h(px(28.0))
            .flex()
            .items_center()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(TOOLBAR))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(if emphasized { TEXT } else { MUTED_TEXT }))
                    .when(emphasized, |element| {
                        element.font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(status),
            )
            .when(
                self.flamegraph_zoom != flamelens::flame::ROOT_ID,
                |element| {
                    element.child(toolbar_button(
                        "Show All",
                        cx.listener(|view, _, _, cx| {
                            view.flamegraph_zoom = flamelens::flame::ROOT_ID;
                            cx.notify();
                        }),
                    ))
                },
            )
            .child(
                div()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .child(metric_toggle(
                        "Cycles",
                        !self.flamegraph_instructions,
                        has_cycles,
                        cx.listener(|view, _, _, cx| {
                            view.flamegraph_instructions = false;
                            view.flamegraph_zoom = flamelens::flame::ROOT_ID;
                            view.selected_stack =
                                view.selected_function.as_deref().and_then(|function| {
                                    view.flamegraph_data()
                                        .and_then(|data| data.hottest_stack_named(false, function))
                                });
                            cx.notify();
                        }),
                    ))
                    .child(metric_toggle(
                        "Instructions",
                        self.flamegraph_instructions,
                        has_instructions,
                        cx.listener(|view, _, _, cx| {
                            view.flamegraph_instructions = true;
                            view.flamegraph_zoom = flamelens::flame::ROOT_ID;
                            view.selected_stack =
                                view.selected_function.as_deref().and_then(|function| {
                                    view.flamegraph_data()
                                        .and_then(|data| data.hottest_stack_named(true, function))
                                });
                            cx.notify();
                        }),
                    )),
            )
    }
}

struct FlameFrameTooltip {
    name: String,
    total: String,
    self_value: String,
}

impl Render for FlameFrameTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(520.0))
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_xs()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .text_color(rgb(MUTED_TEXT))
                    .child(self.total.clone())
                    .child("·")
                    .child(self.self_value.clone()),
            )
    }
}

fn empty_flamegraph(message: &'static str) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED_TEXT))
        .child(message)
}

fn filtered_flamegraph_message(message: impl Into<String>, error: bool) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
        .child(message.into())
}

fn metric_toggle(
    label: &'static str,
    selected: bool,
    available: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "flamegraph-{}",
            label.to_ascii_lowercase()
        )))
        .h(px(18.0))
        .flex()
        .items_center()
        .px_1()
        .rounded_sm()
        .text_xs()
        .when(selected, |element| {
            element
                .bg(rgb(SELECTION))
                .text_color(rgb(0xffffff))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |element| element.text_color(rgb(MUTED_TEXT)))
        .when(available, |element| {
            element
                .cursor_pointer()
                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                .on_click(on_click)
        })
        .when(!available, |element| element.opacity(0.4))
        .child(label)
}

fn toolbar_button(
    label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "flamegraph-action-{}",
            label.to_ascii_lowercase().replace(' ', "-")
        )))
        .h(px(20.0))
        .flex()
        .items_center()
        .px_1()
        .rounded_sm()
        .bg(rgb(SURFACE))
        .cursor_pointer()
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
        .on_click(on_click)
        .child(label)
}
