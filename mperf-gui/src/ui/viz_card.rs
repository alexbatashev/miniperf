use gpui::{
    AnyElement, App, FontWeight, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};

use super::theme::ActiveTheme;

/// The compact panel the analysis views are built from: hairline border,
/// 10px padding, an uppercase caption and an optional trailing action.
#[derive(IntoElement)]
pub struct VizCard {
    title: Option<SharedString>,
    action: Option<AnyElement>,
    children: Vec<AnyElement>,
}

pub fn viz_card(title: impl Into<SharedString>) -> VizCard {
    VizCard {
        title: Some(title.into()),
        action: None,
        children: Vec::new(),
    }
}

impl VizCard {
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl ParentElement for VizCard {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for VizCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .min_w(px(0.0))
            .rounded(theme.radius_lg())
            .border_1()
            .border_color(theme.foreground.opacity(0.1))
            .bg(theme.card)
            .p(px(10.0))
            .text_color(theme.card_foreground)
            .when_some(self.title, |el, title| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.muted_foreground)
                                .child(title.to_uppercase()),
                        )
                        .children(self.action),
                )
            })
            .children(self.children)
    }
}
