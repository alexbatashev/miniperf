use gpui::{
    App, BoxShadow, ElementId, FontWeight, IntoElement, MouseButton, RenderOnce, SharedString,
    Window, div, point, prelude::*, px,
};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

type SelectHandler = Box<dyn Fn(usize, &mut Window, &mut App)>;

pub struct TabItem {
    pub label: SharedString,
    pub icon: Option<Icon>,
    pub mono: bool,
    pub closable: bool,
}

impl TabItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            mono: false,
            closable: false,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }
}

/// shadcn Tabs list, dense variant: h-24px triggers, active = `bg-background`
/// and shadow-sm (dark: input tint). Supports closable (source) tabs; overflow
/// scrolls horizontally.
#[derive(IntoElement)]
pub struct TabBar {
    id: ElementId,
    items: Vec<TabItem>,
    active: usize,
    on_select: Option<SelectHandler>,
    on_close: Option<SelectHandler>,
}

pub fn tab_bar(id: impl Into<ElementId>, items: Vec<TabItem>, active: usize) -> TabBar {
    TabBar {
        id: id.into(),
        items,
        active,
        on_select: None,
        on_close: None,
    }
}

impl TabBar {
    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }

    pub fn on_close(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let on_select = self.on_select.map(std::rc::Rc::new);
        let on_close = self.on_close.map(std::rc::Rc::new);
        let active = self.active;

        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(2.0))
            .h(px(28.0))
            .overflow_x_scroll()
            .children(self.items.into_iter().enumerate().map(|(ix, item)| {
                let selected = ix == active;
                let theme = theme.clone();
                let on_select = on_select.clone();
                let on_close = on_close.clone();

                let mut tab = div()
                    .id(ix)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(24.0))
                    .rounded(theme.radius_md())
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .cursor_pointer();

                tab = if item.closable {
                    tab.pl(px(8.0)).pr(px(4.0))
                } else {
                    tab.px(px(8.0))
                };

                tab = if selected {
                    let styled = tab.text_color(theme.foreground);
                    if theme.dark {
                        styled
                            .bg(theme.input.opacity(0.3))
                            .border_color(theme.input)
                    } else {
                        styled.bg(theme.background).shadow(vec![BoxShadow {
                            color: gpui::black().opacity(0.1),
                            offset: point(px(0.0), px(1.0)),
                            blur_radius: px(3.0),
                            spread_radius: px(0.0),
                        }])
                    }
                } else {
                    tab.text_color(theme.foreground.opacity(0.6))
                        .hover(|s| s.text_color(theme.foreground))
                };

                if let Some(on_select) = on_select.clone() {
                    tab = tab.on_click(move |_, window, cx| on_select(ix, window, cx));
                }

                tab.when_some(item.icon, |el, ic| {
                    let color = if selected {
                        theme.foreground
                    } else {
                        theme.foreground.opacity(0.6)
                    };
                    el.child(icon(ic).size(px(12.0)).color(color))
                })
                .child(
                    div()
                        .when(item.mono, |el| el.font_family(theme.font_mono.clone()))
                        .child(item.label),
                )
                .when(item.closable, |el| {
                    let theme = theme.clone();
                    el.child(
                        div()
                            .id("close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radius_sm())
                            .p(px(2.0))
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.muted))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when_some(on_close, |el, on_close| {
                                el.on_click(move |_, window, cx| on_close(ix, window, cx))
                            })
                            .child(icon(Icon::X).size(px(12.0)).color(theme.muted_foreground)),
                    )
                })
            }))
    }
}
