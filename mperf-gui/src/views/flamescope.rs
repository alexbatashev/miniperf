use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};

use crate::{
    MperfGui,
    profile::TimeRange,
    profile_analysis::FlameScopeHeatmap,
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SURFACE, TEXT, WORKSPACE,
        flame_frame_color,
    },
};

const MAX_COLUMNS: usize = 100;
const ROW_HEIGHT: f32 = 18.0;
const MIN_GRID_HEIGHT: f32 = 220.0;
const MIN_GRID_WIDTH: f32 = 920.0;

impl MperfGui {
    pub(crate) fn render_flamescope_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return flamescope_message("Open a recording to inspect its sample density.", false);
        };
        let profile = &model.profile;
        if let Some(error) = profile.error.clone() {
            return flamescope_message(error, true);
        }

        // Keep the folded time domain visible while a time range is selected,
        // but apply every other global constraint so all central views agree.
        let Some(heatmap) = self.cached_flamescope(MAX_COLUMNS) else {
            return flamescope_message(
                "This recording has no timestamped samples for FlameScope.",
                false,
            );
        };
        if heatmap.total_samples == 0 {
            return flamescope_message(
                "This recording has no timestamped samples for FlameScope.",
                false,
            );
        }

        let selected_range = self.selected_time_range;
        let effective_row_height = (MIN_GRID_HEIGHT / heatmap.rows.max(1) as f32).max(ROW_HEIGHT);
        let grid_height = heatmap.rows as f32 * effective_row_height;
        let header = self.render_flamescope_header(&heatmap, selected_range, cx);
        let axis = div()
            .h(px(30.0))
            .min_h(px(30.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(CHROME))
            .text_xs()
            .text_color(rgb(MUTED_TEXT))
            .child(div().w(px(76.0)).px_2().child("SECOND"))
            .child(
                div()
                    .min_w(px(MIN_GRID_WIDTH))
                    .flex_1()
                    .flex()
                    .justify_between()
                    .px_1()
                    .child("0 ms")
                    .child("250 ms")
                    .child("500 ms")
                    .child("750 ms")
                    .child("1,000 ms"),
            );

        let heatmap_for_paint = heatmap.clone();
        let heatmap_for_click = heatmap.clone();
        let entity = cx.entity();
        let grid = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_heatmap(bounds, &heatmap_for_paint, selected_range, window);

                let heatmap = heatmap_for_click.clone();
                let entity = entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
                    if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                        return;
                    }
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if width <= 0.0 || height <= 0.0 {
                        return;
                    }
                    let x = f32::from(event.position.x - bounds.origin.x).clamp(0.0, width);
                    let y = f32::from(event.position.y - bounds.origin.y).clamp(0.0, height);
                    let column = ((x / width) * heatmap.columns as f32)
                        .floor()
                        .min(heatmap.columns.saturating_sub(1) as f32)
                        as usize;
                    let row = ((y / height) * heatmap.rows as f32)
                        .floor()
                        .min(heatmap.rows.saturating_sub(1) as f32)
                        as usize;
                    let Some(range) = heatmap
                        .selected_ranges(
                            row..row.saturating_add(1),
                            column..column.saturating_add(1),
                        )
                        .into_iter()
                        .next()
                    else {
                        return;
                    };

                    entity.update(cx, |view, cx| {
                        view.select_time_range(range);
                        cx.notify();
                    });
                });
            },
        )
        .size_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(axis)
            .child(
                div()
                    .id("flamescope-scroll")
                    .min_h(px(0.0))
                    .flex_1()
                    .overflow_scroll()
                    .bg(rgb(WORKSPACE))
                    .child(
                        div()
                            .min_w(px(MIN_GRID_WIDTH + 76.0))
                            .h(px(grid_height))
                            .flex()
                            .child(
                                div()
                                    .w(px(76.0))
                                    .min_w(px(76.0))
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(CHROME))
                                    .children((0..heatmap.rows).map(|row| {
                                        div()
                                            .h(px(effective_row_height))
                                            .min_h(px(effective_row_height))
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .pr_2()
                                            .border_b_1()
                                            .border_color(rgb(BORDER))
                                            .text_xs()
                                            .text_color(rgb(MUTED_TEXT))
                                            .child(format!("+{row}s"))
                                    })),
                            )
                            .child(
                                div()
                                    .min_w(px(MIN_GRID_WIDTH))
                                    .h_full()
                                    .flex_1()
                                    .child(grid),
                            ),
                    ),
            )
    }

    fn render_flamescope_header(
        &self,
        heatmap: &FlameScopeHeatmap,
        selected_range: Option<TimeRange>,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = format!(
            "{} samples · {} one-second rows · {} bins/row · {} per bin",
            heatmap.total_samples,
            heatmap.rows,
            heatmap.columns,
            format_duration(heatmap.bin_width_ns),
        );

        div()
            .h(px(30.0))
            .min_h(px(30.0))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Sample density"),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(status),
            )
            .when_some(selected_range, |element, range| {
                element
                    .child(
                        div()
                            .max_w(px(310.0))
                            .truncate()
                            .text_xs()
                            .text_color(rgb(TEXT))
                            .child(format!(
                                "Selected {}",
                                format_range(range, heatmap.range.start_ns)
                            )),
                    )
                    .child(
                        div()
                            .id("flamescope-clear-filter")
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .hover(|element| element.bg(rgb(HOVER)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.clear_global_filter();
                                cx.notify();
                            }))
                            .child("Clear Filters"),
                    )
            })
            .when(
                selected_range.is_none() && self.hotspot_filter.is_some(),
                |element| {
                    element
                        .when_some(self.selected_function.clone(), |element, function| {
                            element.child(
                                div()
                                    .max_w(px(280.0))
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(TEXT))
                                    .child(format!("Function: {function}")),
                            )
                        })
                        .child(
                            div()
                                .id("flamescope-clear-function-filter")
                                .h(px(20.0))
                                .flex()
                                .items_center()
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_xs()
                                .text_color(rgb(ACCENT))
                                .hover(|element| element.bg(rgb(HOVER)))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.clear_global_filter();
                                    cx.notify();
                                }))
                                .child("Clear Filter"),
                        )
                },
            )
    }
}

fn paint_heatmap(
    bounds: Bounds<gpui::Pixels>,
    heatmap: &FlameScopeHeatmap,
    selected_range: Option<TimeRange>,
    window: &mut gpui::Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 || heatmap.rows == 0 || heatmap.columns == 0 {
        return;
    }

    let cell_width = width / heatmap.columns as f32;
    let cell_height = height / heatmap.rows as f32;
    for row in 0..heatmap.rows {
        for column in 0..heatmap.columns {
            let samples = heatmap
                .bin(row, column)
                .map(|bin| bin.samples)
                .unwrap_or_default();
            let origin = point(
                bounds.origin.x + px(column as f32 * cell_width),
                bounds.origin.y + px(row as f32 * cell_height),
            );
            let cell = Bounds::new(
                origin,
                size(
                    px((cell_width - 1.0).max(1.0)),
                    px((cell_height - 1.0).max(1.0)),
                ),
            );
            let range = cell_time_range(heatmap, row, column);
            let selected = selected_range
                .zip(range)
                .is_some_and(|(selected, cell)| ranges_overlap(selected, cell));
            if selected {
                window.paint_quad(fill(cell, rgb(ACCENT)));
                window.paint_quad(fill(
                    cell.inset(px(2.0)),
                    rgb(heat_color(samples, heatmap.max_samples)),
                ));
            } else {
                window.paint_quad(fill(cell, rgb(heat_color(samples, heatmap.max_samples))));
            }
        }
    }
}

fn cell_time_range(heatmap: &FlameScopeHeatmap, row: usize, column: usize) -> Option<TimeRange> {
    heatmap
        .selected_ranges(row..row.saturating_add(1), column..column.saturating_add(1))
        .into_iter()
        .next()
}

fn heat_color(samples: u64, max_samples: u64) -> u32 {
    if samples == 0 || max_samples == 0 {
        return SURFACE;
    }
    let intensity = ((samples as f64).ln_1p() / (max_samples as f64).ln_1p()) as f32;
    let hot = flame_frame_color("FlameScope samples", 2);
    mix_rgb(SURFACE, hot, (0.18 + intensity * 0.82).clamp(0.0, 1.0))
}

fn mix_rgb(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let from = ((from >> shift) & 0xff_u32) as f32;
        let to = ((to >> shift) & 0xff_u32) as f32;
        (from + (to - from) * amount).round().clamp(0.0, 255.0) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn ranges_overlap(left: TimeRange, right: TimeRange) -> bool {
    left.start_ns < right.end_ns && right.start_ns < left.end_ns
}

fn format_range(range: TimeRange, origin_ns: u64) -> String {
    let start = range.start_ns.saturating_sub(origin_ns);
    let end = range.end_ns.saturating_sub(origin_ns);
    format!(
        "{}–{} from start ({})",
        format_duration(start),
        format_duration(end),
        format_duration(range.end_ns.saturating_sub(range.start_ns))
    )
}

fn format_duration(nanoseconds: u64) -> String {
    if nanoseconds >= 1_000_000_000 {
        format!("{:.3}s", nanoseconds as f64 / 1_000_000_000.0)
    } else if nanoseconds >= 1_000_000 {
        format!("{:.1}ms", nanoseconds as f64 / 1_000_000.0)
    } else if nanoseconds >= 1_000 {
        format!("{:.1}µs", nanoseconds as f64 / 1_000.0)
    } else {
        format!("{nanoseconds}ns")
    }
}

fn flamescope_message(message: impl Into<String>, error: bool) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
        .child(message.into())
}
