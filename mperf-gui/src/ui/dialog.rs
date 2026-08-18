use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, ElementId, FontWeight, IntoElement, MouseButton, RenderOnce,
    SharedString, Window, deferred, div, point, prelude::*, px,
};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Modal dialog over a dimmed scrim. Render it as a child of the window
/// root; open state lives in the parent view.
#[derive(IntoElement)]
pub struct Dialog {
    id: ElementId,
    title: SharedString,
    description: Option<SharedString>,
    width: f32,
    on_close: Option<CloseHandler>,
    children: Vec<AnyElement>,
}

pub fn dialog(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Dialog {
    Dialog {
        id: id.into(),
        title: title.into(),
        description: None,
        width: 384.0,
        on_close: None,
        children: Vec::new(),
    }
}

impl Dialog {
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let on_close = self.on_close.clone();
        let scrim_close = on_close.clone();

        deferred(
            div()
                .id(self.id)
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.1))
                .occlude()
                .when_some(scrim_close, |el, on_close| {
                    el.on_mouse_down(MouseButton::Left, move |_, window, cx| on_close(window, cx))
                })
                .child(
                    div()
                        .relative()
                        .w(px(self.width))
                        .max_w(px(560.0))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .p(px(16.0))
                        .rounded(theme.radius_xl())
                        .border_1()
                        .border_color(theme.foreground.opacity(0.1))
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .shadow(vec![BoxShadow {
                            color: gpui::black().opacity(0.1),
                            offset: point(px(0.0), px(10.0)),
                            blur_radius: px(15.0),
                            spread_radius: px(-3.0),
                        }])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(self.title),
                                )
                                .when_some(self.description, |el, description| {
                                    el.child(
                                        div()
                                            .text_size(px(14.0))
                                            .text_color(theme.muted_foreground)
                                            .child(description),
                                    )
                                }),
                        )
                        .children(self.children)
                        .when_some(on_close, |el, on_close| {
                            el.child(
                                div()
                                    .id("dialog-close")
                                    .absolute()
                                    .top(px(8.0))
                                    .right(px(8.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(28.0))
                                    .rounded(theme.radius_md())
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.muted))
                                    .on_click(move |_, window, cx| on_close(window, cx))
                                    .child(
                                        icon(Icon::X).size(px(16.0)).color(theme.muted_foreground),
                                    ),
                            )
                        }),
                ),
        )
        .with_priority(2)
    }
}
