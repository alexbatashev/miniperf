use gpui::{App, Div, FontWeight, SharedString, Stateful, Window, div, prelude::*, px};

use super::theme::ActiveTheme;

#[derive(Clone)]
pub struct Column {
    pub label: SharedString,
    pub width: Option<f32>,
    pub right: bool,
    pub sortable: bool,
}

impl Column {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            width: None,
            right: false,
            sortable: false,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }
}

/// Header row for a virtual table: h-28, 10.5px medium muted labels, sticky
/// by construction (rendered as a sibling above the `uniform_list`).
/// `sort` is `(column_index, descending)`.
pub fn table_header(columns: &[Column], sort: Option<(usize, bool)>, cx: &App) -> Div {
    let theme = cx.theme().clone();
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(28.0))
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .children(columns.iter().enumerate().map(|(ix, column)| {
            let sorted = sort.filter(|(sort_ix, _)| *sort_ix == ix);
            let cell = div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .px(px(8.0))
                .h_full()
                .whitespace_nowrap()
                .when(column.right, |el| el.justify_end())
                .when(column.sortable, |el| el.cursor_pointer())
                .child(column.label.clone())
                .when_some(sorted, |el, (_, descending)| {
                    el.child(
                        div()
                            .text_size(px(9.0))
                            .child(if descending { "▼" } else { "▲" }),
                    )
                });
            match column.width {
                Some(width) => cell.w(px(width)).flex_none(),
                None => cell.flex_1().min_w(px(0.0)),
            }
        }))
}

/// `table_header` with per-column sort clicks: clicking a sortable column
/// invokes `on_sort` with its index.
pub fn table_header_sortable(
    columns: &[Column],
    sort: Option<(usize, bool)>,
    on_sort: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme().clone();
    div()
        .id("table-header")
        .flex()
        .flex_none()
        .items_center()
        .h(px(28.0))
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .children(columns.iter().enumerate().map(|(ix, column)| {
            let sorted = sort.filter(|(sort_ix, _)| *sort_ix == ix);
            let cell = div()
                .id(ix)
                .flex()
                .items_center()
                .gap(px(2.0))
                .px(px(8.0))
                .h_full()
                .whitespace_nowrap()
                .when(column.right, |el| el.justify_end())
                .when(column.sortable, |el| {
                    let on_sort = on_sort.clone();
                    el.cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .on_click(move |_, window, cx| on_sort(ix, window, cx))
                })
                .child(column.label.clone())
                .when_some(sorted, |el, (_, descending)| {
                    el.child(
                        div()
                            .text_size(px(9.0))
                            .child(if descending { "▼" } else { "▲" }),
                    )
                });
            match column.width {
                Some(width) => cell.w(px(width)).flex_none(),
                None => cell.flex_1().min_w(px(0.0)),
            }
        }))
}

/// Row container: 24px, hairline separator, hover wash, `bg-accent` when
/// selected. Compose cells with `table_cell`.
pub fn table_row(selected: bool, cx: &App) -> Div {
    let theme = cx.theme().clone();
    div()
        .flex()
        .items_center()
        .h(px(24.0))
        .w_full()
        .border_b_1()
        .border_color(theme.border.opacity(0.4))
        .cursor_pointer()
        .when(selected, |el| el.bg(theme.accent))
        .when(!selected, |el| el.hover(|s| s.bg(theme.muted.opacity(0.5))))
}

pub fn table_cell(column: &Column) -> Div {
    let cell = div()
        .px(px(8.0))
        .h_full()
        .flex()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .when(column.right, |el| el.justify_end());
    match column.width {
        Some(width) => cell.w(px(width)).flex_none(),
        None => cell.flex_1().min_w(px(0.0)),
    }
}
