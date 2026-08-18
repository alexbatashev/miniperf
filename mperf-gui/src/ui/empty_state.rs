use gpui::{AnyElement, App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

/// The single way any view renders "no data": centered icon + one-line
/// reason + optional action.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Icon,
    reason: SharedString,
    action: Option<AnyElement>,
}

pub fn empty_state(ic: Icon, reason: impl Into<SharedString>) -> EmptyState {
    EmptyState {
        icon: ic,
        reason: reason.into(),
        action: None,
    }
}

impl EmptyState {
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .child(icon(self.icon).size(px(24.0)).color(theme.muted_foreground))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(self.reason),
            )
            .when_some(self.action, |el, action| el.child(action))
    }
}
