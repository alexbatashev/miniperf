use gpui::{AnyView, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

/// Plain-text tooltip view: `bg-foreground text-background` per shadcn.
pub struct TextTooltip {
    text: SharedString,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .max_w(px(320.0))
            .px(px(12.0))
            .py(px(6.0))
            .rounded(theme.radius_md())
            .bg(theme.foreground)
            .text_color(theme.background)
            .text_size(px(12.0))
            .child(self.text.clone())
    }
}

/// Builder for `.tooltip(...)` on any stateful element.
pub fn text_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        let text = text.clone();
        cx.new(|_| TextTooltip { text }).into()
    }
}

/// The (i) icon with a hover tooltip — the Top-Down metric-description
/// pattern from the prototype.
pub fn info_tooltip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex_none()
        .child(
            icon(Icon::Info)
                .size(px(12.0))
                .color(cx.theme().muted_foreground),
        )
        .tooltip(text_tooltip(text))
}
