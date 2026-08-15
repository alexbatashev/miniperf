//! Flame scope: the recording folded into one column per second, with
//! sub-second offset running up the y axis. Heat is sample density; dragging
//! across the x axis sets the global time filter.

use std::sync::Arc;

use gpui::{
    Bounds, CursorStyle, DispatchPhase, Entity, HitboxBehavior, MouseMoveEvent, canvas, fill,
    point, prelude::*, px, size,
};

use super::ShellView;
use crate::charts::{PlotFrame, heat, shape_label};
use crate::profile_analysis::FlameScopeHeatmap;
use crate::ui::Theme;

const GUTTER: f32 = 46.0;
const PAD_TOP: f32 = 6.0;
const PAD_BOTTOM: f32 = 18.0;
/// Height of one sub-fold bin. The grid keeps this cell height instead of
/// stretching to the pane, so a coarse fold stays a heatmap rather than
/// becoming a row of tall bars.
const CELL_H: f32 = 14.0;

/// One cell readout under the cursor.
#[derive(Clone, Copy, PartialEq)]
pub struct ScopeHover {
    pub fold_start_ns: u64,
    pub fold_ns: u64,
    pub offset_ns: u64,
    pub samples: u64,
    pub x: f32,
    pub y: f32,
}

impl ScopeHover {
    /// "0.34s +2.0ms · 12 samples"
    pub fn label(&self) -> String {
        format!(
            "{} +{} · {} samples",
            format_fold_time(self.fold_start_ns, self.fold_ns),
            format_offset_ms(self.offset_ns),
            self.samples
        )
    }
}

/// Human-readable fold size for the view caption: "20 ms", "1 s".
pub fn format_fold_period(fold_ns: u64) -> String {
    if fold_ns >= 1_000_000_000 {
        format!("{} s", fold_ns / 1_000_000_000)
    } else {
        format!("{} ms", fold_ns / 1_000_000)
    }
}

/// Start of a fold column, with just enough precision for the fold size.
pub fn format_fold_time(time_ns: u64, fold_ns: u64) -> String {
    let seconds = time_ns as f64 / 1e9;
    match fold_ns {
        fold if fold >= 1_000_000_000 => format!("{seconds:.0}s"),
        fold if fold >= 100_000_000 => format!("{seconds:.1}s"),
        fold if fold >= 10_000_000 => format!("{seconds:.2}s"),
        _ => format!("{seconds:.3}s"),
    }
}

fn format_offset_ms(offset_ns: u64) -> String {
    let ms = offset_ns as f64 / 1e6;
    if ms >= 10.0 || ms == 0.0 {
        format!("{ms:.0}ms")
    } else {
        format!("{ms:.1}ms")
    }
}

#[derive(Clone)]
pub struct ScopeView {
    pub heatmap: Arc<FlameScopeHeatmap>,
    /// Committed time filter in seconds from the recording start.
    pub selection: Option<(f64, f64)>,
    pub brush: crate::charts::Brush,
    pub duration: f64,
}

impl ScopeView {
    /// Natural height of the grid: one `CELL_H` row per sub-fold bin.
    pub fn height(&self) -> f32 {
        self.heatmap.columns as f32 * CELL_H + PAD_TOP + PAD_BOTTOM
    }

    fn cell_at(
        &self,
        frame: &PlotFrame,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<(usize, usize)> {
        let heatmap = &self.heatmap;
        let plot_height = (frame.height() - PAD_TOP - PAD_BOTTOM).max(1.0);
        let cell_w = frame.width() / heatmap.rows.max(1) as f32;
        let cell_h = plot_height / heatmap.columns.max(1) as f32;
        let x = f32::from(position.x) - frame.left();
        let y = f32::from(position.y) - frame.top() - PAD_TOP;
        if x < 0.0 || y < 0.0 || y > plot_height {
            return None;
        }
        let row = (x / cell_w) as usize;
        let column = ((plot_height - y) / cell_h) as usize;
        (row < heatmap.rows && column < heatmap.columns).then_some((row, column))
    }
}

pub fn scope_canvas(entity: Entity<ShellView>, theme: Theme, view: ScopeView) -> impl IntoElement {
    let height = view.height();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            window.paint_quad(fill(bounds, theme.viz.surface));

            let frame = PlotFrame::new(bounds, GUTTER);
            let heatmap = &view.heatmap;
            let plot_height = (frame.height() - PAD_TOP - PAD_BOTTOM).max(1.0);
            let plot_bottom = frame.top() + PAD_TOP + plot_height;
            let cell_w = frame.width() / heatmap.rows.max(1) as f32;
            let cell_h = plot_height / heatmap.columns.max(1) as f32;
            let max = heatmap.max_samples.max(1) as f32;

            for row in 0..heatmap.rows {
                for column in 0..heatmap.columns {
                    let Some(bin) = heatmap.bin(row, column) else {
                        continue;
                    };
                    if bin.samples == 0 {
                        continue;
                    }
                    let x = frame.left() + row as f32 * cell_w;
                    let y = plot_bottom - (column + 1) as f32 * cell_h;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(px(x), px(y)),
                            size(px(cell_w.max(0.5)), px(cell_h.max(0.5))),
                        ),
                        heat(bin.samples as f32 / max),
                    ));
                }
            }

            let tick_every = match heatmap.rows {
                rows if rows > 30 => 5,
                rows if rows > 15 => 2,
                _ => 1,
            };
            for row in (0..=heatmap.rows).step_by(tick_every) {
                let label = format_fold_time(row as u64 * heatmap.fold_ns, heatmap.fold_ns);
                let line = shape_label(&label, 9.0, theme.viz.muted, window);
                let _ = line.paint(
                    point(
                        px(frame.left() + row as f32 * cell_w - 8.0),
                        px(plot_bottom + 4.0),
                    ),
                    px(10.0),
                    window,
                    cx,
                );
            }

            for fraction in [0.0f32, 0.5, 1.0] {
                let label = format_offset_ms(
                    (fraction as f64 * heatmap.fold_ns as f64).round() as u64,
                );
                let line = shape_label(&label, 9.0, theme.viz.muted, window);
                let y = plot_bottom - fraction * plot_height;
                let _ = line.paint(
                    point(px(frame.left() - 40.0), px(y.min(plot_bottom - 6.0))),
                    px(10.0),
                    window,
                    cx,
                );
            }

            if let Some(range) = view.brush.shown(view.selection) {
                frame.paint_selection(
                    range,
                    view.duration,
                    frame.top() + PAD_TOP,
                    plot_height,
                    &theme,
                    window,
                );
            }

            let hover_entity = entity.clone();
            let hover_hitbox = hitbox.clone();
            let hover_view = view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hovered = hover_hitbox
                    .is_hovered(window)
                    .then(|| hover_view.cell_at(&frame, event.position))
                    .flatten()
                    .and_then(|(row, column)| {
                        let bin = hover_view.heatmap.bin(row, column)?;
                        Some(ScopeHover {
                            offset_ns: column as u64 * hover_view.heatmap.bin_width_ns,
                            fold_start_ns: row as u64 * hover_view.heatmap.fold_ns,
                            fold_ns: hover_view.heatmap.fold_ns,
                            samples: bin.samples,
                            x: f32::from(event.position.x) - f32::from(bounds.left()),
                            y: f32::from(event.position.y) - f32::from(bounds.top()),
                        })
                    });
                hover_entity.update(cx, |this, cx| {
                    let changed = match (this.scope_hover, hovered) {
                        (Some(current), Some(next)) => {
                            current.fold_start_ns != next.fold_start_ns
                                || current.offset_ns != next.offset_ns
                        }
                        (current, next) => current.is_some() != next.is_some(),
                    };
                    if changed {
                        this.scope_hover = hovered;
                        cx.notify();
                    }
                });
            });

            super::timeline::brush(&entity, frame, &hitbox, view.duration, window);
        },
    )
    .flex_none()
    .w_full()
    .h(px(height))
}
