use gpui::{App, Hsla, IntoElement, RenderOnce, Window, div, prelude::*, px, relative};

use super::theme::ActiveTheme;

/// Progress/meter bar: 4px track on `muted`, filled with `primary` or a
/// status color.
#[derive(IntoElement)]
pub struct Meter {
    value: f32,
    color: Option<Hsla>,
}

pub fn meter(value: f32) -> Meter {
    Meter {
        value: value.clamp(0.0, 1.0),
        color: None,
    }
}

impl Meter {
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Meter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let fill = self.color.unwrap_or(theme.primary);
        div()
            .h(px(4.0))
            .w_full()
            .rounded_full()
            .bg(theme.muted)
            .overflow_hidden()
            .child(
                div()
                    .h_full()
                    .w(relative(self.value))
                    .rounded_full()
                    .bg(fill),
            )
    }
}

/// Horizontal multi-segment bar (the TMA mini-bar): segments of (share,
/// color), shares summing to ~1, 8px tall, 2px radius, 1px gaps.
#[derive(IntoElement)]
pub struct SegmentBar {
    segments: Vec<(f32, Hsla)>,
    height: f32,
}

pub fn segment_bar(segments: Vec<(f32, Hsla)>) -> SegmentBar {
    SegmentBar {
        segments,
        height: 8.0,
    }
}

impl SegmentBar {
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
}

impl RenderOnce for SegmentBar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let last = self.segments.len().saturating_sub(1);
        div()
            .flex()
            .h(px(self.height))
            .w_full()
            .rounded(px(2.0))
            .overflow_hidden()
            .children(
                self.segments
                    .into_iter()
                    .enumerate()
                    .map(|(i, (share, color))| {
                        div()
                            .w(relative(share.max(0.0)))
                            .h_full()
                            .bg(color)
                            .when(i != last, |el| el.mr(px(1.0)))
                    }),
            )
    }
}
