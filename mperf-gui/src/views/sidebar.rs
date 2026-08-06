use gpui::{Context, Div, IntoElement, MouseButton, SharedString, div, prelude::*, px, rgb};

use crate::{
    LeftPaneKind, MperfGui, result_name, same_directory,
    theme::{ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED},
};

impl MperfGui {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        match self.active_left_pane {
            LeftPaneKind::Recordings => self.render_recordings_sidebar(cx),
            LeftPaneKind::AvailableViews => self.render_available_views_sidebar(cx),
        }
    }

    fn render_recordings_sidebar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(CHROME))
            .child(
                div()
                    .h(px(26.0))
                    .flex()
                    .items_end()
                    .px_2()
                    .pb_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child("RECENT"),
            )
            .when(self.recent_results.is_empty(), |element| {
                element.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child("No recent recordings"),
                )
            })
            .children(self.recent_results.iter().enumerate().map(|(index, path)| {
                let active = self
                    .model
                    .as_ref()
                    .is_some_and(|model| same_directory(&model.result_directory, path));
                let name = result_name(path);
                let parent = path
                    .parent()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                let path = path.clone();

                div()
                    .id(SharedString::from(format!("recent-recording-{index}")))
                    .min_h(px(44.0))
                    .flex()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .cursor_pointer()
                    .text_sm()
                    .when(active, |element| element.bg(rgb(SELECTION_MUTED)))
                    .when(!active, |element| {
                        element.hover(|element| element.bg(rgb(HOVER)))
                    })
                    .child(
                        div()
                            .w(px(3.0))
                            .when(active, |element| element.bg(rgb(ACCENT))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_1()
                            .flex_col()
                            .justify_center()
                            .px_2()
                            .gap_0p5()
                            .child(div().truncate().child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .child(parent),
                            ),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.load_result_directory(path.clone(), cx);
                    }))
            }))
            .child(div().flex_1())
            .when_some(self.load_error.clone(), |element, error| {
                element.child(
                    div()
                        .p_2()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .text_xs()
                        .text_color(rgb(ERROR))
                        .child(error),
                )
            })
    }

    fn render_available_views_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let available_visualizations = self.available_visualizations();
        let is_empty = available_visualizations.is_empty();

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(CHROME))
            .child(
                div()
                    .h(px(26.0))
                    .flex()
                    .items_end()
                    .px_2()
                    .pb_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child("AVAILABLE VIEWS"),
            )
            .children(available_visualizations.into_iter().map(|visualization| {
                let active = self.active_source.is_none()
                    && self.active_visualization == Some(visualization);
                let open = self.open_visualizations.contains(&visualization);

                div()
                    .id(SharedString::from(format!(
                        "available-view-{}",
                        visualization.id()
                    )))
                    .min_h(px(30.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .cursor_pointer()
                    .text_sm()
                    .when(active, |element| element.bg(rgb(SELECTION_MUTED)))
                    .when(!active, |element| {
                        element.hover(|element| element.bg(rgb(HOVER)))
                    })
                    .child(
                        div()
                            .w(px(3.0))
                            .h_full()
                            .when(active, |element| element.bg(rgb(ACCENT))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .px_2()
                            .child(visualization.title()),
                    )
                    .when(open, |element| {
                        element.child(div().pr_2().text_xs().text_color(rgb(ACCENT)).child("●"))
                    })
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_visualization(visualization);
                        cx.notify();
                    }))
            }))
            .when(is_empty, |element| {
                element.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child(if self.model.is_some() {
                            "No visualizations are available for this recording"
                        } else {
                            "Open a recording to see its available visualizations"
                        }),
                )
            })
            .child(div().flex_1())
    }

    pub(crate) fn render_sidebar_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("left-pane-resizer")
            .w(px(3.0))
            .h_full()
            .bg(rgb(BORDER))
            .cursor_col_resize()
            .hover(|element| element.bg(rgb(ACCENT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.sidebar_resizing = true;
                    cx.notify();
                }),
            )
    }
}
