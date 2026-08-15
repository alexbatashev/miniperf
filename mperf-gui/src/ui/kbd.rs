use gpui::{App, FontWeight, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::theme::ActiveTheme;

/// shadcn Kbd: 20px pill on `muted` with 12px medium muted text. Use
/// `inverted()` inside tooltips (which render on `foreground`).
#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
    inverted: bool,
}

pub fn kbd(keys: impl Into<SharedString>) -> Kbd {
    Kbd {
        keys: keys.into(),
        inverted: false,
    }
}

impl Kbd {
    pub fn inverted(mut self) -> Self {
        self.inverted = true;
        self
    }
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, color) = if self.inverted {
            (theme.background.opacity(0.2), theme.background)
        } else {
            (theme.muted, theme.muted_foreground)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(px(20.0))
            .min_w(px(20.0))
            .px(px(4.0))
            .rounded(theme.radius_sm())
            .bg(bg)
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .whitespace_nowrap()
            .child(self.keys)
    }
}

/// Row of related keys with 4px gaps.
pub fn kbd_group(keys: impl IntoIterator<Item = &'static str>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children(keys.into_iter().map(kbd))
}
