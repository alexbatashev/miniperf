use gpui::{
    AnyElement, App, ElementId, FontWeight, IntoElement, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use super::icon::{Icon, icon};
use super::theme::ActiveTheme;

type ToggleHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// Uppercase section caption: `text-[10.5px] font-medium uppercase
/// tracking-wide text-muted-foreground`.
pub fn section_caption(label: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(label.into().to_uppercase())
}

/// Collapsible section: full-width caption row with a chevron; open state
/// lives in the parent view.
#[derive(IntoElement)]
pub struct CollapsibleSection {
    id: ElementId,
    label: SharedString,
    open: bool,
    on_toggle: Option<ToggleHandler>,
    children: Vec<AnyElement>,
}

pub fn collapsible_section(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    open: bool,
) -> CollapsibleSection {
    CollapsibleSection {
        id: id.into(),
        label: label.into(),
        open,
        on_toggle: None,
        children: Vec::new(),
    }
}

impl CollapsibleSection {
    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl ParentElement for CollapsibleSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for CollapsibleSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let chevron = if self.open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(24.0))
                    .px(px(8.0))
                    .bg(theme.muted.opacity(0.4))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted))
                    .when_some(self.on_toggle, |el, handler| {
                        el.on_click(move |_, window, cx| handler(window, cx))
                    })
                    .child(icon(chevron).size(px(12.0)).color(theme.muted_foreground))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child(self.label.to_uppercase()),
                    ),
            )
            .when(self.open, |el| el.children(self.children))
    }
}
