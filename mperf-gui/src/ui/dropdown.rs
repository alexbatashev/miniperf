use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, ElementId, IntoElement, RenderOnce, SharedString, Window, anchored,
    deferred, div, point, prelude::*, px, relative,
};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

pub struct DropdownItem {
    pub label: SharedString,
    pub trailing: Option<SharedString>,
    pub mono: bool,
    pub checked: Option<bool>,
    pub separator_before: bool,
}

impl DropdownItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            trailing: None,
            mono: false,
            checked: None,
            separator_before: false,
        }
    }

    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn separator_before(mut self) -> Self {
        self.separator_before = true;
        self
    }
}

/// shadcn dropdown menu. Stateless: `open` and the item set live in the
/// parent view; the menu renders via `deferred(anchored(...))` below the
/// trigger and dismisses through `on_toggle(false)` on outside click.
/// Checkbox items stay open on click; plain items close.
#[derive(IntoElement)]
pub struct DropdownMenu {
    id: ElementId,
    trigger: AnyElement,
    open: bool,
    min_width: f32,
    items: Vec<DropdownItem>,
    on_toggle: Option<ToggleHandler>,
    on_select: Option<SelectHandler>,
}

pub fn dropdown_menu(
    id: impl Into<ElementId>,
    trigger: impl IntoElement,
    open: bool,
) -> DropdownMenu {
    DropdownMenu {
        id: id.into(),
        trigger: trigger.into_any_element(),
        open,
        min_width: 128.0,
        items: Vec::new(),
        on_toggle: None,
        on_select: None,
    }
}

impl DropdownMenu {
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    pub fn items(mut self, items: Vec<DropdownItem>) -> Self {
        self.items = items;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for DropdownMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let open = self.open;
        let on_toggle = self.on_toggle.clone();
        let on_select = self.on_select.clone();

        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id(self.id)
                    .cursor_pointer()
                    .when_some(on_toggle.clone(), |el, on_toggle| {
                        el.on_click(move |_, window, cx| on_toggle(!open, window, cx))
                    })
                    .child(self.trigger),
            )
            .when(open, |el| {
                el.child(
                    div().absolute().top(relative(1.0)).left_0().child(
                        deferred(
                            anchored().snap_to_window_with_margin(px(8.0)).child(
                                div()
                                    .occlude()
                                    .mt(px(4.0))
                                    .min_w(px(self.min_width))
                                    .p(px(4.0))
                                    .rounded(theme.radius_lg())
                                    .border_1()
                                    .border_color(theme.foreground.opacity(0.1))
                                    .bg(theme.popover)
                                    .shadow(vec![BoxShadow {
                                        color: gpui::black().opacity(0.1),
                                        offset: point(px(0.0), px(4.0)),
                                        blur_radius: px(6.0),
                                        spread_radius: px(-1.0),
                                    }])
                                    .flex()
                                    .flex_col()
                                    .when_some(on_toggle.clone(), |el, on_toggle| {
                                        el.on_mouse_down_out(move |_, window, cx| {
                                            on_toggle(false, window, cx)
                                        })
                                    })
                                    .children(self.items.into_iter().enumerate().flat_map(
                                        |(ix, item)| {
                                            let theme = theme.clone();
                                            let on_select = on_select.clone();
                                            let on_toggle = on_toggle.clone();
                                            let is_checkbox = item.checked.is_some();

                                            let mut out: Vec<AnyElement> = Vec::new();
                                            if item.separator_before {
                                                out.push(
                                                    div()
                                                        .my(px(4.0))
                                                        .mx(px(-4.0))
                                                        .h(px(1.0))
                                                        .bg(theme.border)
                                                        .into_any_element(),
                                                );
                                            }
                                            out.push(
                                                div()
                                                    .id(ix)
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .px(px(6.0))
                                                    .py(px(4.0))
                                                    .rounded(theme.radius_md())
                                                    .text_size(px(12.0))
                                                    .text_color(theme.popover_foreground)
                                                    .cursor_pointer()
                                                    .hover(|s| {
                                                        s.bg(theme.accent)
                                                            .text_color(theme.accent_foreground)
                                                    })
                                                    .on_click(move |_, window, cx| {
                                                        if let Some(on_select) = &on_select {
                                                            on_select(ix, window, cx);
                                                        }
                                                        if !is_checkbox
                                                            && let Some(on_toggle) = &on_toggle
                                                        {
                                                            on_toggle(false, window, cx);
                                                        }
                                                    })
                                                    .when(is_checkbox, |el| {
                                                        let checked = item.checked.unwrap_or(false);
                                                        el.child(
                                                            div()
                                                                .w(px(16.0))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .when(checked, |el| {
                                                                    el.child(
                                                                        icon(Icon::Check)
                                                                            .size(px(12.0))
                                                                            .color(
                                                                                theme.foreground,
                                                                            ),
                                                                    )
                                                                }),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .when(item.mono, |el| {
                                                                el.font_family(
                                                                    theme.font_mono.clone(),
                                                                )
                                                            })
                                                            .child(item.label),
                                                    )
                                                    .when_some(item.trailing, |el, trailing| {
                                                        el.child(
                                                            div()
                                                                .text_size(px(10.0))
                                                                .text_color(theme.muted_foreground)
                                                                .child(trailing),
                                                        )
                                                    })
                                                    .into_any_element(),
                                            );
                                            out
                                        },
                                    )),
                            ),
                        )
                        .with_priority(1),
                    ),
                )
            })
    }
}
