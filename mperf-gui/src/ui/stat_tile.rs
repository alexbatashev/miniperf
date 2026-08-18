use gpui::{App, FontWeight, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::theme::ActiveTheme;

/// Selection-panel stat tile: centered, `bg-muted/50`, 9px uppercase label
/// over a semibold tabular value.
#[derive(IntoElement)]
pub struct StatTile {
    label: SharedString,
    value: SharedString,
}

pub fn stat_tile(label: impl Into<SharedString>, value: impl Into<SharedString>) -> StatTile {
    StatTile {
        label: label.into(),
        value: value.into(),
    }
}

impl RenderOnce for StatTile {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(theme.muted.opacity(0.5))
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(theme.muted_foreground)
                    .child(self.label.to_uppercase()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(self.value),
            )
    }
}
