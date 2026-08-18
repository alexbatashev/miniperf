use gpui::{
    App, ElementId, IntoElement, MouseButton, RenderOnce, SharedString, Window, div, prelude::*, px,
};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// Filter-bar chip: `h-6 rounded-md border px-2 text-[11px]`, optionally
/// active (series-1 tint) and closable.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    leading_icon: Option<Icon>,
    mono: bool,
    active: bool,
    on_close: Option<CloseHandler>,
}

pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Chip {
    Chip {
        id: id.into(),
        label: label.into(),
        leading_icon: None,
        mono: false,
        active: false,
        on_close: None,
    }
}

impl Chip {
    pub fn icon(mut self, icon: Icon) -> Self {
        self.leading_icon = Some(icon);
        self
    }

    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let accent = theme.viz.series[0];

        let mut base = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .h(px(24.0))
            .px(px(8.0))
            .rounded(theme.radius_md())
            .border_1()
            .text_size(px(11.0));

        base = if self.active {
            base.border_color(accent.opacity(0.4))
                .bg(accent.opacity(0.1))
                .text_color(theme.foreground)
        } else {
            base.border_color(theme.border)
                .bg(theme.background)
                .text_color(theme.muted_foreground)
        };

        base.when_some(self.leading_icon, |el, ic| {
            el.child(icon(ic).size(px(12.0)).color(cx.theme().muted_foreground))
        })
        .child(
            div()
                .when(self.mono, |el| el.font_family(theme.font_mono.clone()))
                .child(self.label),
        )
        .when_some(self.on_close, |el, handler| {
            el.child(
                div()
                    .id("close")
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .p(px(1.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| handler(window, cx))
                    .child(icon(Icon::X).size(px(12.0)).color(theme.muted_foreground)),
            )
        })
    }
}
