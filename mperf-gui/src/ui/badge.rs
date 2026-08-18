use gpui::{
    App, Div, FontWeight, Hsla, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};

use super::theme::ActiveTheme;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BadgeVariant {
    #[default]
    Default,
    Secondary,
    Destructive,
    Outline,
    Ghost,
}

#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    variant: BadgeVariant,
    color: Option<Hsla>,
}

pub fn badge(label: impl Into<SharedString>) -> Badge {
    Badge {
        label: label.into(),
        variant: BadgeVariant::default(),
        color: None,
    }
}

impl Badge {
    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Tinted badge: `color` at 15% for the fill, full for the text — the
    /// module/scenario badge pattern from the prototype.
    pub fn tint(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let base = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .h(px(20.0))
            .px(px(8.0))
            .rounded_full()
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .overflow_hidden();

        let styled: Div = if let Some(color) = self.color {
            base.bg(color.opacity(0.15)).text_color(color)
        } else {
            match self.variant {
                BadgeVariant::Default => {
                    base.bg(theme.primary).text_color(theme.primary_foreground)
                }
                BadgeVariant::Secondary => base
                    .bg(theme.secondary)
                    .text_color(theme.secondary_foreground),
                BadgeVariant::Destructive => {
                    let bg = if theme.dark {
                        theme.destructive.opacity(0.2)
                    } else {
                        theme.destructive.opacity(0.1)
                    };
                    base.bg(bg).text_color(theme.destructive)
                }
                BadgeVariant::Outline => {
                    base.border_color(theme.border).text_color(theme.foreground)
                }
                BadgeVariant::Ghost => base.text_color(theme.foreground),
            }
        };

        styled.child(self.label)
    }
}
