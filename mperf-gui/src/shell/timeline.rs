//! Master-timeline canvases: per-thread activity lanes and pinned counter
//! tracks over the full recording range, with a shared drag brush that sets
//! the global time filter.

use std::{collections::BTreeSet, sync::Arc};

use gpui::{
    Bounds, CursorStyle, Entity, HitboxBehavior, Window, canvas, fill, point, prelude::*, px, size,
};

use super::ShellView;
use super::session::{ShellSession, format_count};
use crate::charts::{
    PlotFrame, paint_area_series, register_time_brush, shape_label, truncate_label,
};
use crate::profile_analysis::CounterAggregation;
use crate::ui::Theme;

/// Socket-wide counters live in their own track group: they follow the time
/// filter but ignore thread and symbol scoping.
pub fn is_system_track(key: &str) -> bool {
    let key = key.to_lowercase();
    [
        "uncore", "imc", "dram", "rapl", "energy", "socket", "package",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

pub const LANE_H: f32 = 15.0;
pub const AXIS_H: f32 = 16.0;
pub const GUTTER: f32 = 92.0;
pub const TRACK_H: f32 = 38.0;
pub const MAX_LANES: usize = 16;

/// Everything the timeline canvases need for one frame.
#[derive(Clone)]
pub struct TimelineView {
    pub session: Arc<ShellSession>,
    pub disabled_threads: BTreeSet<u32>,
    /// Committed time filter in seconds from the recording start.
    pub selection: Option<(f64, f64)>,
    /// In-flight brush preview, overriding `selection` while dragging.
    pub preview: Option<(f64, f64)>,
}

impl TimelineView {
    fn duration(&self) -> f64 {
        self.session.duration_seconds().max(1e-9)
    }

    fn shown_range(&self) -> Option<(f64, f64)> {
        self.preview.or(self.selection)
    }
}

pub fn visible_lanes(session: &ShellSession) -> usize {
    session.threads.len().min(MAX_LANES)
}

pub fn lanes_height(session: &ShellSession) -> f32 {
    let extra = if session.threads.len() > MAX_LANES {
        1
    } else {
        0
    };
    (visible_lanes(session) + extra) as f32 * LANE_H + AXIS_H + 4.0
}

/// Registers the shared brush on a canvas that spans the full time axis.
pub(super) fn brush(
    entity: &Entity<ShellView>,
    frame: PlotFrame,
    hitbox: &gpui::Hitbox,
    duration: f64,
    window: &mut Window,
) {
    register_time_brush(
        entity,
        frame,
        hitbox,
        duration,
        window,
        |this: &mut ShellView| &mut this.brush,
        |this: &mut ShellView, range, cx| this.commit_time_filter(range, cx),
    );
}

/// Thread-activity lanes with the drag brush. Density maps to alpha against
/// the global bucket maximum; filtered-out threads render dimmed.
pub fn lanes_canvas(
    entity: Entity<ShellView>,
    theme: Theme,
    view: TimelineView,
) -> impl IntoElement {
    let height = lanes_height(&view.session);
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            window.paint_quad(fill(bounds, theme.viz.surface));

            let session = &view.session;
            let duration = view.duration();
            let frame = PlotFrame::new(bounds, GUTTER);
            let px0 = frame.left();
            let pw = frame.width();
            let shown = visible_lanes(session);
            let mut lane_bottom = shown as f32 * LANE_H;

            if let Some(lanes) = session.lanes.as_ref() {
                let buckets = lanes.buckets;
                let bar_w = pw / buckets as f32;
                for (ix, thread) in session.threads.iter().take(shown).enumerate() {
                    let y = f32::from(bounds.top()) + ix as f32 * LANE_H;
                    let enabled = !view.disabled_threads.contains(&thread.thread_id);
                    let label_color = if enabled {
                        theme.viz.ink_2
                    } else {
                        theme.viz.muted
                    };
                    let label = truncate_label(&thread.label, 13);
                    let line = shape_label(&label, 10.0, label_color, window);
                    let _ = line.paint(
                        point(bounds.left() + px(6.0), px(y + 1.5)),
                        px(12.0),
                        window,
                        cx,
                    );

                    for (bucket, count) in lanes.lanes[ix].counts.iter().enumerate() {
                        if *count == 0 {
                            continue;
                        }
                        let density = *count as f32 / lanes.max_count as f32;
                        let alpha = if enabled { 0.15 + 0.85 * density } else { 0.12 };
                        let rect = Bounds::new(
                            point(px(px0 + bucket as f32 * bar_w), px(y + 1.0)),
                            size(px(bar_w.ceil().max(1.0)), px(LANE_H - 3.0)),
                        );
                        window.paint_quad(fill(rect, theme.viz.series[0].opacity(alpha)));
                    }
                }
                if session.threads.len() > shown {
                    let y = f32::from(bounds.top()) + shown as f32 * LANE_H;
                    let hidden = session.threads.len() - shown;
                    let note = format!("+{hidden} more threads (filter via the threads menu)");
                    let line = shape_label(&note, 9.0, theme.viz.muted, window);
                    let _ = line.paint(
                        point(bounds.left() + px(6.0), px(y + 1.5)),
                        px(12.0),
                        window,
                        cx,
                    );
                    lane_bottom += LANE_H;
                }
            }

            let axis_y = f32::from(bounds.top()) + lane_bottom + 2.0;
            frame.paint_time_axis(axis_y, duration, &theme, window, cx);

            if let Some(range) = view.shown_range() {
                frame.paint_selection(range, duration, frame.top(), lane_bottom, &theme, window);
            }

            brush(&entity, frame, &hitbox, duration, window);
        },
    )
    .flex_none()
    .w_full()
    .h(px(height))
}

fn format_track_value(aggregation: CounterAggregation, value: f64) -> String {
    match aggregation {
        CounterAggregation::Ratio => format!("{value:.2}"),
        CounterAggregation::Sum => format_count(value),
    }
}

/// One counter track: 92px gutter (label over last value) + area plot of
/// per-bin sums, with gaps where the recording has no data.
pub fn track_canvas(
    entity: Entity<ShellView>,
    theme: Theme,
    view: TimelineView,
    track_index: usize,
    height: f32,
) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            window.paint_quad(fill(bounds, theme.viz.surface));

            let duration = view.duration();
            let Some(tracks) = view.session.tracks.as_ref() else {
                return;
            };
            let Some(track) = tracks.tracks.get(track_index) else {
                return;
            };

            let frame = PlotFrame::new(bounds, GUTTER);
            let px0 = frame.left();
            let pw = frame.width();
            let h = frame.height();
            let top = frame.top();

            let label = truncate_label(&track.label, 14);
            let line = shape_label(&label, 10.0, theme.viz.ink_2, window);
            let _ = line.paint(
                point(bounds.left() + px(6.0), px(top + 4.0)),
                px(12.0),
                window,
                cx,
            );
            if let Some(last) = track.values.iter().rev().find_map(|value| *value) {
                let value_line = shape_label(
                    &format_track_value(track.aggregation, last),
                    10.0,
                    theme.viz.muted,
                    window,
                );
                let _ = value_line.paint(
                    point(bounds.left() + px(6.0), px(top + 18.0)),
                    px(12.0),
                    window,
                    cx,
                );
            }

            let max = track
                .values
                .iter()
                .flatten()
                .fold(0f64, |max, value| max.max(*value));
            let bins = track.values.len().max(1);
            let step = pw / bins as f32;
            let points: Vec<(f32, Option<f64>)> = track
                .values
                .iter()
                .enumerate()
                .map(|(bin, value)| (px0 + (bin as f32 + 0.5) * step, *value))
                .collect();
            paint_area_series(
                top + 3.0,
                (h - 7.0).max(1.0),
                &points,
                max,
                theme.viz.series[0],
                window,
            );

            if let Some(range) = view.shown_range() {
                frame.paint_selection(range, duration, top, h, &theme, window);
            }

            brush(&entity, frame, &hitbox, duration, window);
        },
    )
    .flex_none()
    .w_full()
    .h(px(height))
}
