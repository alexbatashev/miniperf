use gpui::{App, Div, FontWeight, div, prelude::*, px};

use super::theme::ActiveTheme;

/// Card surface: radius 14, ring-style hairline, vertical padding 16.
/// Compose header/content with `card_title`/`card_description` or arbitrary
/// children padded via `card_section`.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .rounded(theme.radius_xl())
        .border_1()
        .border_color(theme.foreground.opacity(0.1))
        .bg(theme.card)
        .py(px(16.0))
        .text_size(px(14.0))
        .text_color(theme.card_foreground)
        .overflow_hidden()
}

/// Horizontal padding wrapper for card children.
pub fn card_section() -> Div {
    div().px(px(16.0))
}

pub fn card_title(cx: &App) -> Div {
    card_section()
        .text_size(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().card_foreground)
}

pub fn card_description(cx: &App) -> Div {
    card_section()
        .text_size(px(14.0))
        .text_color(cx.theme().muted_foreground)
}
