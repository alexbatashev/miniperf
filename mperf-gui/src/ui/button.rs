use gpui::{
    App, ClickEvent, Div, ElementId, FontWeight, IntoElement, RenderOnce, SharedString, Stateful,
    Window, div, prelude::*, px,
};

use super::icon::{Icon, icon};
use super::theme::{ActiveTheme, Theme, mix};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Outline,
    Secondary,
    Ghost,
    Destructive,
    Link,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    Xs,
    Lg,
    Icon,
    IconSm,
    IconXs,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            Self::Default | Self::Icon => 32.0,
            Self::Sm | Self::IconSm => 28.0,
            Self::Xs | Self::IconXs => 24.0,
            Self::Lg => 36.0,
        }
    }

    fn is_icon(self) -> bool {
        matches!(self, Self::Icon | Self::IconSm | Self::IconXs)
    }

    fn padding_x(self) -> f32 {
        match self {
            Self::Default | Self::Sm | Self::Lg => 10.0,
            Self::Xs => 8.0,
            _ => 0.0,
        }
    }

    fn gap(self) -> f32 {
        match self {
            Self::Default | Self::Lg => 6.0,
            _ => 4.0,
        }
    }

    fn radius(self, theme: &Theme) -> gpui::Pixels {
        match self {
            Self::Default | Self::Lg | Self::Icon => theme.radius_lg(),
            _ => theme.radius_md(),
        }
    }

    fn text_size(self) -> f32 {
        match self {
            Self::Xs | Self::IconXs => 12.0,
            Self::Sm | Self::IconSm => 12.8,
            _ => 14.0,
        }
    }

    fn icon_size(self) -> f32 {
        match self {
            Self::Xs | Self::IconXs => 12.0,
            Self::Sm => 14.0,
            _ => 16.0,
        }
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    leading_icon: Option<Icon>,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    active: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

pub fn button(id: impl Into<ElementId>) -> Button {
    Button {
        id: id.into(),
        label: None,
        leading_icon: None,
        variant: ButtonVariant::default(),
        size: ButtonSize::default(),
        disabled: false,
        active: false,
        on_click: None,
    }
}

impl Button {
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.leading_icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Keeps the hover appearance on, e.g. while an attached menu is open.
    pub fn toggled(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let size = self.size;

        let mut base = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(px(size.height()))
            .gap(px(size.gap()))
            .rounded(size.radius(&theme))
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(px(size.text_size()))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap();

        base = if size.is_icon() {
            base.w(px(size.height()))
        } else {
            base.px(px(size.padding_x()))
        };

        base = apply_variant(base, self.variant, &theme, self.active);

        if self.disabled {
            base = base.opacity(0.5);
        } else {
            base = base.cursor_pointer().active(|s| s.top(px(1.0)));
            if let Some(handler) = self.on_click {
                base = base.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }

        base.relative()
            .when_some(self.leading_icon, |el, ic| {
                let color = icon_color(self.variant, &theme);
                el.child(icon(ic).size(px(size.icon_size())).color(color))
            })
            .when_some(self.label, |el, label| el.child(label))
    }
}

fn icon_color(variant: ButtonVariant, theme: &Theme) -> gpui::Hsla {
    match variant {
        ButtonVariant::Default => theme.primary_foreground,
        ButtonVariant::Secondary => theme.secondary_foreground,
        ButtonVariant::Destructive => theme.destructive,
        ButtonVariant::Link => theme.primary,
        _ => theme.foreground,
    }
}

fn apply_variant(
    base: Stateful<Div>,
    variant: ButtonVariant,
    theme: &Theme,
    active: bool,
) -> Stateful<Div> {
    match variant {
        ButtonVariant::Default => base
            .bg(theme.primary)
            .text_color(theme.primary_foreground)
            .hover(|s| s.bg(theme.primary.opacity(0.8))),
        ButtonVariant::Outline => {
            let hover_bg = theme.muted;
            let styled = base
                .bg(if active { hover_bg } else { theme.background })
                .border_color(if theme.dark {
                    theme.input
                } else {
                    theme.border
                })
                .text_color(theme.foreground);
            styled.hover(move |s| s.bg(hover_bg))
        }
        ButtonVariant::Secondary => {
            let hover_bg = mix(theme.secondary, theme.foreground, 0.05);
            base.bg(theme.secondary)
                .text_color(theme.secondary_foreground)
                .hover(move |s| s.bg(hover_bg))
        }
        ButtonVariant::Ghost => {
            let hover_bg = if theme.dark {
                theme.muted.opacity(0.5)
            } else {
                theme.muted
            };
            let styled = if active {
                base.bg(hover_bg).text_color(theme.foreground)
            } else {
                base.text_color(theme.foreground)
            };
            styled.hover(move |s| s.bg(hover_bg))
        }
        ButtonVariant::Destructive => {
            let (bg, hover_bg) = if theme.dark {
                (
                    theme.destructive.opacity(0.2),
                    theme.destructive.opacity(0.3),
                )
            } else {
                (
                    theme.destructive.opacity(0.1),
                    theme.destructive.opacity(0.2),
                )
            };
            base.bg(bg)
                .text_color(theme.destructive)
                .hover(move |s| s.bg(hover_bg))
        }
        ButtonVariant::Link => base.text_color(theme.primary).hover(|s| s.underline()),
    }
}
