use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, Canvas, ContentMask, Context, CursorStyle, DispatchPhase, Entity, Hitbox,
    HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point,
    ScrollHandle, SharedString, Window, canvas, fill, point, px, rgb, size,
};

use crate::{
    HoveredFrame, MperfGui,
    flamegraph::FlameLayout,
    profile_analysis::{CallTree, IcicleLayout},
    theme::{FLAME_FRAME_HEIGHT, SELECTION, SURFACE, TEXT, WORKSPACE, flame_frame_color},
};

/// Frames narrower than this many pixels are merged into muted filler runs.
const MIN_FRAME_WIDTH: f32 = 0.5;
/// Frames narrower than this many pixels get no label.
const MIN_LABEL_WIDTH: f32 = 36.0;
const LABEL_FONT_SIZE: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelStyle {
    Percent,
    InclSelf,
}

pub(crate) struct FlameChartFrame {
    /// Stack identifier (plain flamegraph) or call-tree node id.
    pub key: usize,
    /// Interned profile frame id when this chart is sample-backed.
    pub frame_id: Option<usize>,
    pub name: SharedString,
    pub color: u32,
    pub x: f64,
    pub width: f64,
    pub depth: usize,
    pub total: u64,
    pub self_total: u64,
    pub clickable: bool,
}

pub(crate) struct FlameChart {
    pub frames: Vec<FlameChartFrame>,
    /// Frame indices per depth, sorted by x, for painting and hit-testing.
    rows: Vec<Vec<u32>>,
    pub max_depth: usize,
    pub root_at_bottom: bool,
    pub total: u64,
    pub root_name: String,
    /// Icicle focus metadata: whether a non-root node is focused.
    pub focused: bool,
    /// One-shot request to scroll the root row into view after a rebuild.
    scroll_to_root: Cell<bool>,
}

impl FlameChart {
    pub fn from_flame_layout(layout: &FlameLayout) -> Self {
        let frames = layout
            .frames
            .iter()
            .map(|frame| FlameChartFrame {
                key: frame.id,
                frame_id: None,
                color: flame_frame_color(&frame.name, frame.depth),
                name: SharedString::from(frame.name.clone()),
                x: frame.x as f64,
                width: frame.width as f64,
                depth: frame.depth,
                total: frame.value,
                self_total: frame.self_value,
                clickable: frame.id != flamelens::flame::ROOT_ID,
            })
            .collect();
        Self::new(
            frames,
            layout.max_depth,
            layout.total,
            true,
            layout.root_name.clone(),
            false,
        )
    }

    pub fn from_icicle_layout(
        layout: &IcicleLayout,
        tree: &CallTree,
        root_at_bottom: bool,
    ) -> Self {
        let frames = layout
            .frames
            .iter()
            .map(|frame| FlameChartFrame {
                key: frame.node_id,
                frame_id: frame.frame_id,
                color: if frame.frame_id.is_some() {
                    flame_frame_color(&frame.label, frame.depth)
                } else {
                    SURFACE
                },
                name: SharedString::from(frame.label.clone()),
                x: frame.x,
                width: frame.width,
                depth: frame.depth,
                total: frame.inclusive_samples,
                self_total: frame.self_samples,
                clickable: true,
            })
            .collect();
        let root_name = tree.nodes[layout.focus_node].label.clone();
        Self::new(
            frames,
            layout.max_depth,
            layout.total_samples,
            root_at_bottom,
            root_name,
            layout.focus_node != tree.root,
        )
    }

    fn new(
        frames: Vec<FlameChartFrame>,
        max_depth: usize,
        total: u64,
        root_at_bottom: bool,
        root_name: String,
        focused: bool,
    ) -> Self {
        let mut rows = vec![Vec::new(); max_depth + 1];
        for (index, frame) in frames.iter().enumerate() {
            rows[frame.depth].push(index as u32);
        }
        for row in &mut rows {
            row.sort_by(|left, right| {
                frames[*left as usize]
                    .x
                    .total_cmp(&frames[*right as usize].x)
            });
        }
        Self {
            frames,
            rows,
            max_depth,
            root_at_bottom,
            total,
            root_name,
            focused,
            scroll_to_root: Cell::new(true),
        }
    }

    pub fn graph_height(&self) -> f32 {
        ((self.max_depth + 1) as f32 * FLAME_FRAME_HEIGHT).max(180.0)
    }

    fn row_top(&self, depth: usize) -> f32 {
        let visual_row = if self.root_at_bottom {
            self.max_depth - depth
        } else {
            depth
        };
        visual_row as f32 * FLAME_FRAME_HEIGHT
    }

    fn hit_test(
        &self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) -> Option<&FlameChartFrame> {
        if !bounds.contains(&position) {
            return None;
        }
        let visual_row = (f32::from(position.y - bounds.origin.y) / FLAME_FRAME_HEIGHT) as usize;
        if visual_row > self.max_depth {
            return None;
        }
        let depth = if self.root_at_bottom {
            self.max_depth - visual_row
        } else {
            visual_row
        };
        let width = f32::from(bounds.size.width) as f64;
        if width <= 0.0 {
            return None;
        }
        let fx = f32::from(position.x - bounds.origin.x) as f64 / width;
        let row = self.rows.get(depth)?;
        let next = row.partition_point(|index| self.frames[*index as usize].x <= fx);
        let frame = &self.frames[row[next.checked_sub(1)?] as usize];
        (fx < frame.x + frame.width).then_some(frame)
    }
}

pub(crate) fn find_focus_node(tree: &CallTree, frame_path: &[usize]) -> Option<usize> {
    if frame_path.is_empty() {
        return None;
    }
    tree.nodes.iter().find_map(|node| {
        let candidate = tree
            .focus_path(node.id)
            .into_iter()
            .filter_map(|node_id| tree.nodes[node_id].frame_id)
            .collect::<Vec<_>>();
        (candidate == frame_path).then_some(node.id)
    })
}

type ClickHandler = Rc<dyn Fn(&mut MperfGui, &FlameChartFrame, &mut Context<MperfGui>)>;

/// A single-element flame chart. Only frames at least half a pixel wide are
/// painted (narrower neighbors merge into filler runs), labels are shaped only
/// for wide frames, and hover/click use direct hit-testing, so painting cost
/// tracks what is visible instead of the recording size.
#[expect(clippy::too_many_arguments)]
pub(crate) fn flame_chart_canvas(
    chart: Rc<FlameChart>,
    hovered_key: Option<usize>,
    selected_key: Option<usize>,
    selected_name: Option<String>,
    label_style: LabelStyle,
    entity: Entity<MperfGui>,
    scroll_handle: Option<ScrollHandle>,
    on_click: ClickHandler,
) -> Canvas<Hitbox> {
    let paint_chart = chart.clone();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
            // A deep graph is mostly empty tail rows at the top, so the first
            // paint of a fresh chart anchors the scroll position to the root.
            if paint_chart.scroll_to_root.take()
                && paint_chart.root_at_bottom
                && let Some(handle) = scroll_handle.as_ref()
            {
                let overflow = bounds.size.height - window.content_mask().bounds.size.height;
                if overflow > px(0.0) {
                    handle.set_offset(point(px(0.0), -overflow));
                    window.request_animation_frame();
                }
            }
            paint_frames(
                bounds,
                &paint_chart,
                hovered_key,
                selected_key,
                selected_name.as_deref(),
                label_style,
                window,
                cx,
            );

            let hover_chart = paint_chart.clone();
            let hover_entity = entity.clone();
            let hover_hitbox = hitbox.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hit = hover_hitbox
                    .is_hovered(window)
                    .then(|| hover_chart.hit_test(bounds, event.position))
                    .flatten();
                hover_entity.update(cx, |view, cx| {
                    let hovered = hit
                        .filter(|frame| frame.clickable)
                        .map(|frame| HoveredFrame {
                            id: frame.key,
                            name: frame.name.to_string(),
                            value: frame.total,
                            self_value: frame.self_total,
                        });
                    if view.hovered_frame.as_ref().map(|frame| frame.id)
                        != hovered.as_ref().map(|frame| frame.id)
                    {
                        view.hovered_frame = hovered;
                        cx.notify();
                    }
                });
            });

            let click_chart = paint_chart.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let Some(frame) = click_chart.hit_test(bounds, event.position) else {
                    return;
                };
                if !frame.clickable {
                    return;
                }
                entity.update(cx, |view, cx| on_click(view, frame, cx));
            });
        },
    )
}

#[expect(clippy::too_many_arguments)]
fn paint_frames(
    bounds: Bounds<Pixels>,
    chart: &FlameChart,
    hovered_key: Option<usize>,
    selected_key: Option<usize>,
    selected_name: Option<&str>,
    label_style: LabelStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return;
    }
    let mask = window.content_mask().bounds;
    let frame_height = px(FLAME_FRAME_HEIGHT - 1.0);

    for (depth, row) in chart.rows.iter().enumerate() {
        let top = bounds.origin.y + px(chart.row_top(depth));
        if top + frame_height < mask.origin.y || top > mask.origin.y + mask.size.height {
            continue;
        }
        let mut filler: Option<(f32, f32)> = None;
        for index in row {
            let frame = &chart.frames[*index as usize];
            let left = frame.x as f32 * width;
            let frame_width = frame.width as f32 * width;
            if frame_width < MIN_FRAME_WIDTH {
                filler = match filler {
                    Some((start, end)) if left <= end + MIN_FRAME_WIDTH => {
                        Some((start, end.max(left + frame_width)))
                    }
                    Some(run) => {
                        paint_filler(bounds, run, top, frame_height, window);
                        Some((left, left + frame_width))
                    }
                    None => Some((left, left + frame_width)),
                };
                continue;
            }
            if let Some(run) = filler.take() {
                paint_filler(bounds, run, top, frame_height, window);
            }

            let rect = Bounds::new(
                point(bounds.origin.x + px(left), top),
                size(px(frame_width), frame_height),
            );
            let selected = selected_key == Some(frame.key)
                || selected_name.is_some_and(|name| crate::profile_names_match(&frame.name, name));
            let hovered = hovered_key == Some(frame.key);
            let (border_width, border_color) = if selected {
                (2.0, SELECTION)
            } else if hovered {
                (2.0, TEXT)
            } else {
                (1.0, WORKSPACE)
            };
            window.paint_quad(PaintQuad {
                corner_radii: (2.0).into(),
                border_widths: (border_width).into(),
                border_color: rgb(border_color).into(),
                ..fill(rect, rgb(frame.color))
            });

            if frame_width >= MIN_LABEL_WIDTH {
                paint_label(rect, frame, chart.total, label_style, window, cx);
            }
        }
        if let Some(run) = filler {
            paint_filler(bounds, run, top, frame_height, window);
        }
    }
}

fn paint_filler(
    bounds: Bounds<Pixels>,
    (start, end): (f32, f32),
    top: Pixels,
    height: Pixels,
    window: &mut Window,
) {
    let rect = Bounds::new(
        point(bounds.origin.x + px(start), top),
        size(px((end - start).max(MIN_FRAME_WIDTH)), height),
    );
    window.paint_quad(fill(rect, rgb(SURFACE)));
}

fn paint_label(
    rect: Bounds<Pixels>,
    frame: &FlameChartFrame,
    total: u64,
    label_style: LabelStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let percent = if total == 0 {
        0.0
    } else {
        frame.total as f64 / total as f64 * 100.0
    };
    let mut label = match label_style {
        LabelStyle::Percent => format!("{}  {percent:.1}%", frame.name),
        LabelStyle::InclSelf => format!(
            "{} · {} incl / {} self · {percent:.1}%",
            frame.name, frame.total, frame.self_total
        ),
    };
    // Bound shaping cost for very long symbol names; the mask clips the rest.
    let max_chars = (f32::from(rect.size.width) / 5.0) as usize + 4;
    if label.chars().count() > max_chars {
        label.truncate(
            label
                .char_indices()
                .nth(max_chars)
                .map_or(label.len(), |(index, _)| index),
        );
    }

    let style = window.text_style();
    let mut run = style.to_run(label.len());
    run.color = rgb(0xf7fbff).into();
    let line = window.text_system().shape_line(
        SharedString::from(label),
        px(LABEL_FONT_SIZE),
        &[run],
        None,
    );
    let label_rect = Bounds::new(
        point(rect.origin.x + px(4.0), rect.origin.y),
        size(rect.size.width - px(8.0), rect.size.height),
    );
    window.with_content_mask(Some(ContentMask { bounds: label_rect }), |window| {
        let _ = line.paint(label_rect.origin, rect.size.height, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flamegraph::FlamegraphData;
    use flamelens::flame::{FlameGraph, ROOT_ID};

    fn chart() -> FlameChart {
        let graph = FlameGraph::from_string("main;hot 75\nmain;cold 25\n".to_string(), false);
        let data = FlamegraphData {
            cycles: Some(graph),
            instructions: None,
            error: None,
        };
        FlameChart::from_flame_layout(&data.layout(false, ROOT_ID).unwrap())
    }

    fn bounds(width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(width), px(height)))
    }

    #[test]
    fn hit_test_maps_rows_and_proportional_x_positions() {
        let chart = chart();
        assert_eq!(chart.max_depth, 2);
        let bounds = bounds(1000.0, chart.graph_height());

        // Depth 2 is the top visual row because the root sits at the bottom.
        let hot = chart.hit_test(bounds, point(px(100.0), px(4.0))).unwrap();
        assert_eq!(hot.name.as_ref(), "hot");
        assert_eq!(hot.total, 75);
        let cold = chart.hit_test(bounds, point(px(900.0), px(4.0))).unwrap();
        assert_eq!(cold.name.as_ref(), "cold");
        let root = chart
            .hit_test(bounds, point(px(500.0), px(2.0 * FLAME_FRAME_HEIGHT + 4.0)))
            .unwrap();
        assert!(!root.clickable);
        assert!(chart.hit_test(bounds, point(px(500.0), px(-1.0))).is_none());
    }

    #[test]
    fn frames_are_grouped_per_depth_and_sorted_by_x() {
        let chart = chart();
        assert_eq!(chart.rows.len(), 3);
        for row in &chart.rows {
            for pair in row.windows(2) {
                assert!(
                    chart.frames[pair[0] as usize].x <= chart.frames[pair[1] as usize].x,
                    "rows must stay sorted for binary-search hit testing"
                );
            }
        }
    }
}
