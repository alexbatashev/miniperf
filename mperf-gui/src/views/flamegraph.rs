use std::rc::Rc;

use gpui::{Context, Div, FontWeight, SharedString, div, prelude::*, px, rgb};
use num_format::ToFormattedString;

use crate::{
    MperfGui,
    theme::{BORDER, ERROR, HOVER, MUTED_TEXT, SELECTION, SURFACE, TEXT, TOOLBAR},
    views::flame_canvas::{LabelStyle, flame_chart_canvas},
};

impl MperfGui {
    pub(crate) fn render_flamegraph_workspace(&self, cx: &mut Context<Self>) -> Div {
        if self.selected_time_range.is_some() || self.hotspot_filter.is_some() {
            return self.render_filtered_flamegraph(cx);
        }
        if self.flamegraph_data().is_none() {
            return empty_flamegraph("This recording does not include a flamegraph.");
        }
        self.render_flamegraph(cx)
    }

    fn render_filtered_flamegraph(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return empty_flamegraph("Open a recording to inspect its flamegraph.");
        };
        if let Some(error) = model.profile.error.as_ref() {
            return filtered_flamegraph_message(error.clone(), true);
        }

        let Some(chart) = self
            .cached_filtered_flame_chart()
            .filter(|chart| chart.total > 0)
        else {
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
        };

        let status = self
            .hovered_frame
            .as_ref()
            .map(|frame| {
                format!(
                    "{}  ·  {} profile samples total  ·  {} self",
                    frame.name,
                    frame.value.to_formatted_string(&num_format::Locale::en),
                    frame
                        .self_value
                        .to_formatted_string(&num_format::Locale::en)
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{} profile samples · primary-symbol stacks · active global filter",
                    chart.total.to_formatted_string(&num_format::Locale::en)
                )
            });
        let graph_height = chart.graph_height();
        let selected_function = self.selected_function.clone();
        let graph = flame_chart_canvas(
            chart,
            self.hovered_frame.as_ref().map(|frame| frame.id),
            None,
            selected_function,
            LabelStyle::Percent,
            cx.entity(),
            Some(self.filtered_flamegraph_scroll_handle.clone()),
            Rc::new(|view, frame, cx| {
                if let Some(frame_id) = frame.frame_id {
                    view.select_profile_function(frame_id);
                    cx.notify();
                }
            }),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_filtered_flamegraph_header(status, cx))
            .child(
                div()
                    .id("filtered-flamegraph-scroll")
                    .track_scroll(&self.filtered_flamegraph_scroll_handle)
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

    fn render_flamegraph(&self, cx: &mut Context<Self>) -> Div {
        let (has_cycles, has_instructions) = self
            .flamegraph_data()
            .map(|flamegraph| {
                (
                    flamegraph.cycles.is_some(),
                    flamegraph.instructions.is_some(),
                )
            })
            .unwrap_or_default();

        let Some(chart) = self.cached_flame_chart() else {
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
                        self.flamegraph_data()
                            .and_then(|flamegraph| flamegraph.error.clone())
                            .unwrap_or_else(|| "No flamegraph data available".to_string()),
                    ),
            );
        };

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
                chart.root_name,
                chart.total.to_formatted_string(&num_format::Locale::en),
                metric_name
            )
        });
        let header = self.render_flamegraph_header(
            status,
            self.hovered_frame.is_some(),
            has_cycles,
            has_instructions,
            cx,
        );
        let graph_height = chart.graph_height();
        let graph = flame_chart_canvas(
            chart,
            self.hovered_frame.as_ref().map(|frame| frame.id),
            self.selected_stack,
            None,
            LabelStyle::Percent,
            cx.entity(),
            Some(self.flamegraph_scroll_handle.clone()),
            Rc::new(|view, frame, cx| {
                view.select_flame_frame(frame.key, frame.name.to_string());
                cx.notify();
            }),
        );

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("flamegraph-scroll")
                .track_scroll(&self.flamegraph_scroll_handle)
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
