use gpui::{
    div, prelude::*, px, relative, rgb, Context, Div, FontWeight, IntoElement, MouseButton,
    SharedString,
};

use crate::{
    result_name,
    theme::{
        ACCENT, BORDER, CHROME, DOCUMENT_BAR_HEIGHT, HOVER, MUTED_TEXT, STATUS_BAR,
        STATUS_BAR_HEIGHT, SURFACE, TEXT, TITLE_BAR, TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_SPACE,
        WORKSPACE,
    },
    LeftPaneKind, MperfGui,
};

impl MperfGui {
    pub(crate) fn render_title_bar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let recording_name = self
            .model
            .as_ref()
            .map(|model| result_name(&model.result_directory))
            .unwrap_or_else(|| "mperf".to_string());
        let scenario = self
            .model
            .as_ref()
            .map(|model| model.record_info.scenario.name().to_string());

        div()
            .id("title-bar")
            .h(px(TITLE_BAR_HEIGHT))
            .min_h(px(TITLE_BAR_HEIGHT))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(TITLE_BAR))
            .text_sm()
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .child(
                div()
                    .w(relative(1.0 / 3.0))
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(px(TRAFFIC_LIGHT_SPACE))
                    .text_color(rgb(MUTED_TEXT))
                    .child("mperf"),
            )
            .child(
                div()
                    .w(relative(1.0 / 3.0))
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(recording_name),
                    )
                    .when_some(scenario, |element, scenario| {
                        element.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(rgb(MUTED_TEXT))
                                .child(scenario),
                        )
                    }),
            )
            .child(div().w(relative(1.0 / 3.0)).h_full().flex())
    }

    pub(crate) fn render_document_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("document-bar")
            .h(px(DOCUMENT_BAR_HEIGHT))
            .min_h(px(DOCUMENT_BAR_HEIGHT))
            .flex()
            .items_center()
            .overflow_scroll()
            .bg(rgb(CHROME))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(
                self.open_visualizations
                    .iter()
                    .copied()
                    .map(|visualization| {
                        let selected = self.active_source.is_none()
                            && self.active_visualization == Some(visualization);
                        document_tab(visualization.title(), selected)
                            .id(SharedString::from(format!(
                                "document-{}",
                                visualization.id()
                            )))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "close-document-{}",
                                        visualization.id()
                                    )))
                                    .ml_1()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .text_color(rgb(MUTED_TEXT))
                                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.close_visualization(visualization);
                                        cx.notify();
                                    }))
                                    .child("×"),
                            )
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.select_visualization(visualization);
                                cx.notify();
                            }))
                    }),
            )
            .children(
                self.source_documents
                    .iter()
                    .enumerate()
                    .map(|(index, document)| {
                        let selected = self.active_source == Some(index);
                        let title = document.title.clone();
                        document_tab(title, selected)
                            .id(SharedString::from(format!("source-document-{index}")))
                            .child(
                                div()
                                    .id(SharedString::from(format!("close-source-{index}")))
                                    .ml_1()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .text_color(rgb(MUTED_TEXT))
                                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.close_source(index);
                                        cx.notify();
                                    }))
                                    .child("×"),
                            )
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.active_source = Some(index);
                                cx.notify();
                            }))
                    }),
            )
            .child(div().flex_1())
    }

    pub(crate) fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.model.is_none() {
            self.render_result_picker(cx).into_any_element()
        } else if self.active_source.is_some() {
            self.render_source_document(cx).into_any_element()
        } else {
            match self.active_visualization {
                Some(crate::VisualizationKind::Roofline) => {
                    self.render_roofline_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::Flamegraph) => {
                    self.render_flamegraph_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::FlameScope) => {
                    self.render_flamescope_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::Icicle) => {
                    self.render_icicle_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::StackTimeline) => {
                    self.render_stack_timeline_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::CpuHeatmap) => {
                    self.render_cpu_heatmap_workspace(cx).into_any_element()
                }
                Some(crate::VisualizationKind::CallersAndCallees) => self
                    .render_callers_and_callees_workspace(cx)
                    .into_any_element(),
                Some(crate::VisualizationKind::MetricsTimeline) => self
                    .render_metrics_timeline_workspace(cx)
                    .into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .child(
                        div()
                            .mb_2()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("No visualization open"),
                    )
                    .child(
                        div()
                            .text_color(rgb(MUTED_TEXT))
                            .child("Choose one from Available Views in the sidebar."),
                    )
                    .into_any_element(),
            }
        };

        div()
            .id("primary-workspace")
            .min_h(px(0.0))
            .flex_1()
            .overflow_hidden()
            .bg(rgb(WORKSPACE))
            .child(content)
    }

    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selection = self
            .hotspot_filter
            .as_ref()
            .map(|filter| filter.root.clone())
            .or_else(|| self.selected_function.clone());

        div()
            .id("status-bar")
            .h(px(STATUS_BAR_HEIGHT))
            .min_h(px(STATUS_BAR_HEIGHT))
            .flex()
            .items_center()
            .bg(rgb(STATUS_BAR))
            .text_xs()
            .text_color(rgb(MUTED_TEXT))
            .px(px(2.0))
            .gap_1()
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("toggle-recordings")
                            .w(px(22.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(
                                if !self.sidebar_collapsed
                                    && self.active_left_pane == LeftPaneKind::Recordings
                                {
                                    ACCENT
                                } else {
                                    MUTED_TEXT
                                },
                            ))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.toggle_left_pane(LeftPaneKind::Recordings);
                                cx.notify();
                            }))
                            .child("▤"),
                    )
                    .child(
                        div()
                            .id("toggle-available-views")
                            .w(px(22.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(
                                if !self.sidebar_collapsed
                                    && self.active_left_pane == LeftPaneKind::AvailableViews
                                {
                                    ACCENT
                                } else {
                                    MUTED_TEXT
                                },
                            ))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.toggle_left_pane(LeftPaneKind::AvailableViews);
                                cx.notify();
                            }))
                            .child("◫"),
                    ),
            )
            .when_some(selection, |element, selection| {
                element.child(status_item(selection))
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("toggle-statistics-panel")
                    .w(px(24.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(
                        if !self.bottom_panel_collapsed
                            && self.active_bottom_panel == crate::BottomPanelKind::Statistics
                        {
                            ACCENT
                        } else {
                            MUTED_TEXT
                        },
                    ))
                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.toggle_bottom_panel(crate::BottomPanelKind::Statistics);
                        cx.notify();
                    }))
                    .child(crate::BottomPanelKind::Statistics.icon()),
            )
            .child(
                div()
                    .id("toggle-hotspots-panel")
                    .w(px(24.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(
                        if !self.bottom_panel_collapsed
                            && self.active_bottom_panel == crate::BottomPanelKind::Hotspots
                        {
                            ACCENT
                        } else {
                            MUTED_TEXT
                        },
                    ))
                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.toggle_bottom_panel(crate::BottomPanelKind::Hotspots);
                        cx.notify();
                    }))
                    .child(crate::BottomPanelKind::Hotspots.icon()),
            )
    }

    fn render_result_picker(&self, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_sm()
            .child(
                div()
                    .mb_2()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open an mperf recording"),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(MUTED_TEXT))
                    .child("Choose a recent recording or open its result directory."),
            )
            .child(
                div()
                    .id("open-recording")
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .px_4()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .when(!self.picking_directory, |element| {
                        element
                            .cursor_pointer()
                            .hover(|element| element.bg(rgb(HOVER)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.select_result_directory(cx);
                            }))
                    })
                    .child(if self.picking_directory {
                        "Selecting…"
                    } else {
                        "Open Recording…"
                    }),
            )
    }
}

fn document_tab(title: impl Into<SharedString>, selected: bool) -> Div {
    div()
        .h_full()
        .min_w(px(112.0))
        .max_w(px(220.0))
        .flex()
        .items_center()
        .px_2()
        .border_r_1()
        .border_color(rgb(BORDER))
        .cursor_pointer()
        .text_sm()
        .when(selected, |element| {
            element
                .bg(rgb(WORKSPACE))
                .text_color(rgb(TEXT))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |element| {
            element
                .text_color(rgb(MUTED_TEXT))
                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
        })
        .child(div().min_w_0().flex_1().truncate().child(title.into()))
}

fn status_item(text: impl Into<SharedString>) -> Div {
    div()
        .h_full()
        .max_w(px(320.0))
        .flex()
        .items_center()
        .px_1()
        .truncate()
        .child(text.into())
}
