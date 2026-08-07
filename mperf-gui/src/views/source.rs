use gpui::{Context, Div, div, prelude::*, px, rgb};

use crate::{
    MperfGui,
    theme::{ACCENT, BORDER, CHROME, ERROR, MUTED_TEXT, SELECTION_MUTED, SURFACE, TEXT},
};

impl MperfGui {
    pub(crate) fn render_source_document(&self, _cx: &mut Context<Self>) -> Div {
        let Some(index) = self.active_source else {
            return div();
        };
        let Some(document) = self.source_documents.get(index) else {
            return div();
        };

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
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_sm()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(document.path.display().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(format!("Line {}", document.focus_line)),
                    ),
            )
            .when_some(document.error.clone(), |element, error| {
                element.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_color(rgb(ERROR))
                        .child(error),
                )
            })
            .when(document.error.is_none(), |element| {
                element.child(
                    div()
                        .id("source-scroll")
                        .min_h(px(0.0))
                        .flex_1()
                        .overflow_scroll()
                        .track_scroll(&document.scroll_handle)
                        .bg(rgb(SURFACE))
                        .font_family("SF Mono")
                        .text_sm()
                        .children(document.lines.iter().enumerate().map(|(index, line)| {
                            let line_number = index + 1;
                            let focused = line_number == document.focus_line;
                            div()
                                .id(gpui::SharedString::from(format!("source-line-{index}")))
                                .min_w_full()
                                .h(px(22.0))
                                .flex()
                                .items_center()
                                .when(focused, |element| element.bg(rgb(SELECTION_MUTED)))
                                .child(
                                    div()
                                        .w(px(64.0))
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .pr_3()
                                        .border_r_1()
                                        .border_color(rgb(BORDER))
                                        .bg(rgb(CHROME))
                                        .text_color(rgb(if focused { ACCENT } else { MUTED_TEXT }))
                                        .child(line_number.to_string()),
                                )
                                .child(
                                    div()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .pl_3()
                                        .pr_6()
                                        .whitespace_nowrap()
                                        .text_color(rgb(TEXT))
                                        .child(if line.is_empty() {
                                            " ".to_string()
                                        } else {
                                            line.clone()
                                        }),
                                )
                        })),
                )
            })
    }
}
