use gpui::{
    App, ElementId, FontWeight, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};

use super::theme::ActiveTheme;

/// Single-select toggle group in segmented (spacing 0, outline) mode:
/// collapsed inner borders, outer corners rounded, selected item = `bg-muted`.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    height: f32,
    padding_x: f32,
    text_size: f32,
    on_select: Option<Box<dyn Fn(usize, &mut Window, &mut App) + 'static>>,
}

pub fn segmented(
    id: impl Into<ElementId>,
    items: Vec<SharedString>,
    selected: usize,
) -> SegmentedControl {
    SegmentedControl {
        id: id.into(),
        items,
        selected,
        height: 32.0,
        padding_x: 10.0,
        text_size: 14.0,
        on_select: None,
    }
}

impl SegmentedControl {
    /// The 24px variant the view toolbars use (`h-6 px-2 text-[11px]`), so a
    /// toggle group fits inside a 32px toolbar row.
    pub fn compact(mut self) -> Self {
        self.height = 24.0;
        self.padding_x = 8.0;
        self.text_size = 11.0;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let border = if theme.dark {
            theme.input
        } else {
            theme.border
        };
        let on_select = self.on_select.map(std::rc::Rc::new);
        let selected = self.selected;
        let last = self.items.len().saturating_sub(1);
        let (height, padding_x, text_size) = (self.height, self.padding_x, self.text_size);

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .children(self.items.into_iter().enumerate().map(|(ix, label)| {
                let theme = theme.clone();
                let on_select = on_select.clone();
                let is_selected = ix == selected;

                let mut item = div()
                    .id(ix)
                    .flex()
                    .items_center()
                    .h(px(height))
                    .px(px(padding_x))
                    .border_t_1()
                    .border_b_1()
                    .border_r_1()
                    .when(ix == 0, |el| el.border_l_1())
                    .border_color(border)
                    .text_size(px(text_size))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .cursor_pointer();

                if ix == 0 {
                    item = item
                        .rounded_tl(theme.radius_lg())
                        .rounded_bl(theme.radius_lg());
                }
                if ix == last {
                    item = item
                        .rounded_tr(theme.radius_lg())
                        .rounded_br(theme.radius_lg());
                }

                item = if is_selected {
                    item.bg(theme.muted).text_color(theme.foreground)
                } else {
                    item.text_color(theme.muted_foreground)
                        .hover(|s| s.bg(theme.muted.opacity(0.5)).text_color(theme.foreground))
                };

                item.when_some(on_select, |el, on_select| {
                    el.on_click(move |_, window, cx| on_select(ix, window, cx))
                })
                .child(label)
            }))
    }
}
