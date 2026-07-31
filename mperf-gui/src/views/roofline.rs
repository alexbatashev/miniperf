use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, size, Bounds, Context, Div, FontWeight,
    MouseButton, MouseDownEvent, PathBuilder, SharedString,
};

use crate::{
    roofline::{RooflineData, RooflineLoop},
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED, SURFACE, WORKSPACE,
    },
    MperfGui,
};

const HEADER_HEIGHT: f32 = 38.0;
const LOOP_PANEL_WIDTH: f32 = 420.0;
const LOOP_HEADER_HEIGHT: f32 = 26.0;
const LOOP_ROW_HEIGHT: f32 = 46.0;
const GFLOPS_WIDTH: f32 = 84.0;
const AI_WIDTH: f32 = 68.0;
const EFFICIENCY_WIDTH: f32 = 68.0;
const POINT: u32 = 0x6f9ca5;
const GRID: u32 = 0x353538;
const TICK_COUNT: usize = 5;

impl MperfGui {
    pub(crate) fn render_roofline_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(data) = self.roofline_data() else {
            return roofline_message(
                "This recording does not contain a Roofline scenario.",
                false,
            );
        };
        if let Some(error) = data.error.clone() {
            return roofline_message(error, true);
        }

        let plot = RooflinePlot::build(data);
        let plotted_count = plot.points.len();
        let loop_count = data.loops.len();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(self.render_roofline_header(data, plotted_count, loop_count))
            .child(
                div()
                    .min_h(px(0.0))
                    .flex_1()
                    .flex()
                    .child(self.render_roofline_chart(data, plot, cx))
                    .child(self.render_roofline_loop_panel(data, cx)),
            )
    }

    fn render_roofline_header(
        &self,
        data: &RooflineData,
        plotted_count: usize,
        loop_count: usize,
    ) -> impl IntoElement {
        div()
            .h(px(HEADER_HEIGHT))
            .min_h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("FP64 Roofline"),
            )
            .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(format!(
                "{plotted_count} plotted · {loop_count} recorded loops"
            )))
            .child(div().flex_1())
            .when_some(data.calibration.as_ref(), |element, calibration| {
                element
                    .child(metric_chip(
                        "Compute",
                        format!("{:.2} GFLOP/s", calibration.fp64_gflops),
                    ))
                    .child(metric_chip(
                        "Memory",
                        format!("{:.2} GB/s", calibration.memory_gbytes_per_second),
                    ))
                    .child(metric_chip(
                        "Ridge",
                        format!("{:.3} FLOP/B", calibration.ridge_point_flops_per_byte),
                    ))
            })
            .when(data.calibration.is_none(), |element| {
                element.child(
                    div()
                        .rounded_sm()
                        .px_2()
                        .py_1()
                        .bg(rgb(CHROME))
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child("Host calibration was not recorded"),
                )
            })
    }

    fn render_roofline_chart(
        &self,
        data: &RooflineData,
        plot: RooflinePlot,
        cx: &mut Context<Self>,
    ) -> Div {
        if plot.points.is_empty() {
            return div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(rgb(BORDER))
                .p_4()
                .text_sm()
                .text_color(rgb(MUTED_TEXT))
                .child(if data.loops.is_empty() {
                    "No loops were recorded."
                } else {
                    "Recorded loops contain no positive FP64 throughput and arithmetic intensity."
                });
        }

        let selected = self.selected_roofline_loop;
        let plot_for_paint = plot.clone();
        let plot_for_click = plot.clone();
        let entity = cx.entity();
        let graph = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_roofline(bounds, &plot_for_paint, selected, window);

                let plot = plot_for_click.clone();
                let entity = entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
                    if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                        return;
                    }
                    let Some(index) = nearest_point(bounds, &plot, event.position) else {
                        return;
                    };
                    entity.update(cx, |view, cx| {
                        view.select_roofline_loop(index, event.click_count >= 2);
                        cx.notify();
                    });
                });
            },
        )
        .size_full();

        let x_ticks = plot
            .x_ticks()
            .into_iter()
            .map(axis_tick)
            .collect::<Vec<_>>();
        let y_ticks = plot
            .y_ticks()
            .into_iter()
            .rev()
            .map(axis_tick)
            .collect::<Vec<_>>();

        div()
            .min_w(px(0.0))
            .flex_1()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(28.0))
                    .min_h(px(28.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(legend_item(ACCENT, "Calibrated attainable roof"))
                    .child(legend_item(POINT, "Recorded FP64 loop"))
                    .child(div().flex_1())
                    .child("Double-click a point to open source"),
            )
            .child(
                div()
                    .min_h(px(0.0))
                    .flex_1()
                    .flex()
                    .p_3()
                    .child(
                        div()
                            .w(px(72.0))
                            .min_w(px(72.0))
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(18.0))
                                    .min_h(px(18.0))
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .child("GFLOP/s"),
                            )
                            .child(
                                div()
                                    .min_h(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .justify_between()
                                    .pr_2()
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .children(y_ticks),
                            )
                            .child(div().h(px(38.0)).min_h(px(38.0))),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(18.0))
                                    .min_h(px(18.0))
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .child("Performance"),
                            )
                            .child(
                                div()
                                    .min_h(px(180.0))
                                    .flex_1()
                                    .cursor_pointer()
                                    .child(graph),
                            )
                            .child(
                                div()
                                    .h(px(20.0))
                                    .min_h(px(20.0))
                                    .flex()
                                    .justify_between()
                                    .pt_1()
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .children(x_ticks),
                            )
                            .child(
                                div()
                                    .h(px(18.0))
                                    .min_h(px(18.0))
                                    .flex()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .child("Arithmetic intensity (FLOP / byte)"),
                            ),
                    ),
            )
    }

    fn render_roofline_loop_panel(&self, data: &RooflineData, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(LOOP_PANEL_WIDTH))
            .min_w(px(340.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(
                div()
                    .h(px(28.0))
                    .min_h(px(28.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_sm()
                    .child("All loops")
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(MUTED_TEXT))
                            .child(data.loops.len().to_string()),
                    ),
            )
            .child(roofline_loop_table_header())
            .child(
                div()
                    .id("roofline-loop-scroll")
                    .min_h(px(0.0))
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.roofline_loop_scroll_handle)
                    .children(data.loops.iter().enumerate().map(|(index, loop_data)| {
                        self.render_roofline_loop_row(
                            index,
                            loop_data,
                            data,
                            self.selected_roofline_loop == Some(index),
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .h(px(26.0))
                    .min_h(px(26.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child("Single-click selects · double-click opens source"),
            )
    }

    fn render_roofline_loop_row(
        &self,
        index: usize,
        loop_data: &RooflineLoop,
        data: &RooflineData,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let function_name = loop_data.function_name.clone();
        let location = loop_location(loop_data);
        let gflops = loop_data
            .fp64_gflops()
            .map(format_metric)
            .unwrap_or_else(|| "—".to_string());
        let intensity = loop_data
            .fp64_arithmetic_intensity()
            .map(format_metric)
            .unwrap_or_else(|| "—".to_string());
        let efficiency = data
            .calibration
            .as_ref()
            .and_then(|calibration| loop_data.efficiency(calibration))
            .map(|value| format!("{:.1}%", value * 100.0))
            .unwrap_or_else(|| "—".to_string());
        let has_source = loop_data.source().is_some();

        div()
            .id(SharedString::from(format!("roofline-loop-{index}")))
            .h(px(LOOP_ROW_HEIGHT))
            .min_h(px(LOOP_ROW_HEIGHT))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .cursor_pointer()
            .text_sm()
            .when(selected, |element| {
                element
                    .bg(rgb(SELECTION_MUTED))
                    .border_l_2()
                    .border_color(rgb(ACCENT))
            })
            .when(!selected, |element| {
                element.hover(|element| element.bg(rgb(HOVER)))
            })
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                view.select_roofline_loop(index, event.click_count() >= 2);
                cx.notify();
            }))
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_0p5()
                    .px_2()
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(function_name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(location),
                    ),
            )
            .child(numeric_cell(gflops, GFLOPS_WIDTH))
            .child(numeric_cell(intensity, AI_WIDTH))
            .child(numeric_cell(efficiency, EFFICIENCY_WIDTH))
            .child(
                div()
                    .w(px(22.0))
                    .min_w(px(22.0))
                    .flex()
                    .justify_center()
                    .text_color(rgb(if has_source { ACCENT } else { MUTED_TEXT }))
                    .child(if has_source { "↗" } else { "" }),
            )
    }
}

#[derive(Clone, Debug)]
struct RooflinePlot {
    points: Vec<RooflinePoint>,
    calibration: Option<mperf_data::RooflineCalibration>,
    x_min_log: f64,
    x_max_log: f64,
    y_min_log: f64,
    y_max_log: f64,
}

#[derive(Clone, Copy, Debug)]
struct RooflinePoint {
    loop_index: usize,
    intensity: f64,
    gflops: f64,
}

impl RooflinePlot {
    fn build(data: &RooflineData) -> Self {
        let points = data
            .loops
            .iter()
            .enumerate()
            .filter_map(|(loop_index, loop_data)| {
                Some(RooflinePoint {
                    loop_index,
                    intensity: loop_data.fp64_arithmetic_intensity()?,
                    gflops: loop_data.fp64_gflops()?,
                })
            })
            .collect::<Vec<_>>();

        let mut x_values = points
            .iter()
            .map(|point| point.intensity)
            .collect::<Vec<_>>();
        let mut y_values = points.iter().map(|point| point.gflops).collect::<Vec<_>>();
        if let Some(calibration) = data.calibration.as_ref() {
            x_values.push(calibration.ridge_point_flops_per_byte);
            y_values.push(calibration.fp64_gflops);
        }

        let (x_min_log, x_max_log) = log_extent(&x_values, -2.0, 2.0);
        let memory_at_left = data
            .calibration
            .as_ref()
            .map(|calibration| calibration.memory_gbytes_per_second * 10.0_f64.powf(x_min_log))
            .filter(|value| value.is_finite() && *value > 0.0);
        if let Some(value) = memory_at_left {
            y_values.push(value);
        }
        let (y_min_log, y_max_log) = log_extent(&y_values, -1.0, 3.0);

        Self {
            points,
            calibration: data.calibration.clone(),
            x_min_log,
            x_max_log,
            y_min_log,
            y_max_log,
        }
    }

    fn x_ticks(&self) -> Vec<f64> {
        log_ticks(self.x_min_log, self.x_max_log)
    }

    fn y_ticks(&self) -> Vec<f64> {
        log_ticks(self.y_min_log, self.y_max_log)
    }

    fn x_fraction(&self, value: f64) -> f32 {
        log_fraction(value, self.x_min_log, self.x_max_log)
    }

    fn y_fraction(&self, value: f64) -> f32 {
        log_fraction(value, self.y_min_log, self.y_max_log)
    }
}

fn paint_roofline(
    bounds: Bounds<gpui::Pixels>,
    plot: &RooflinePlot,
    selected: Option<usize>,
    window: &mut gpui::Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    window.paint_quad(fill(bounds, rgb(SURFACE)));

    for step in 0..TICK_COUNT {
        let fraction = step as f32 / (TICK_COUNT - 1) as f32;
        let x = bounds.origin.x + px(width * fraction);
        let y = bounds.origin.y + px(height * fraction);
        window.paint_quad(fill(
            Bounds::new(point(x, bounds.origin.y), size(px(1.0), bounds.size.height)),
            rgb(GRID),
        ));
        window.paint_quad(fill(
            Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.0))),
            rgb(GRID),
        ));
    }

    if let Some(calibration) = plot.calibration.as_ref() {
        let mut path = PathBuilder::stroke(px(2.0));
        for step in 0..=64 {
            let fraction = step as f64 / 64.0;
            let log_x = plot.x_min_log + fraction * (plot.x_max_log - plot.x_min_log);
            let intensity = 10.0_f64.powf(log_x);
            let gflops = calibration
                .fp64_gflops
                .min(calibration.memory_gbytes_per_second * intensity);
            let position = plot_position(bounds, plot, intensity, gflops);
            if step == 0 {
                path.move_to(position);
            } else {
                path.line_to(position);
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(ACCENT));
        }
    }

    for point_data in &plot.points {
        let position = plot_position(bounds, plot, point_data.intensity, point_data.gflops);
        let is_selected = selected == Some(point_data.loop_index);
        let outer_size = if is_selected { 12.0 } else { 8.0 };
        window.paint_quad(fill(
            Bounds::new(
                point(
                    position.x - px(outer_size / 2.0),
                    position.y - px(outer_size / 2.0),
                ),
                size(px(outer_size), px(outer_size)),
            ),
            rgb(if is_selected { ACCENT } else { POINT }),
        ));
        if is_selected {
            window.paint_quad(fill(
                Bounds::new(
                    point(position.x - px(2.0), position.y - px(2.0)),
                    size(px(4.0), px(4.0)),
                ),
                rgb(WORKSPACE),
            ));
        }
    }
}

fn nearest_point(
    bounds: Bounds<gpui::Pixels>,
    plot: &RooflinePlot,
    position: gpui::Point<gpui::Pixels>,
) -> Option<usize> {
    plot.points
        .iter()
        .filter_map(|point_data| {
            let point = plot_position(bounds, plot, point_data.intensity, point_data.gflops);
            let dx = f32::from(position.x - point.x);
            let dy = f32::from(position.y - point.y);
            let distance_squared = dx * dx + dy * dy;
            (distance_squared <= 14.0 * 14.0).then_some((point_data.loop_index, distance_squared))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn plot_position(
    bounds: Bounds<gpui::Pixels>,
    plot: &RooflinePlot,
    intensity: f64,
    gflops: f64,
) -> gpui::Point<gpui::Pixels> {
    let x = f32::from(bounds.size.width) * plot.x_fraction(intensity);
    let y = f32::from(bounds.size.height) * (1.0 - plot.y_fraction(gflops));
    point(bounds.origin.x + px(x), bounds.origin.y + px(y))
}

fn log_extent(values: &[f64], default_min: f64, default_max: f64) -> (f64, f64) {
    let mut logs = values
        .iter()
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(f64::log10);
    let Some(first) = logs.next() else {
        return (default_min, default_max);
    };
    let (mut minimum, mut maximum) = (first, first);
    for value in logs {
        minimum = minimum.min(value);
        maximum = maximum.max(value);
    }
    minimum = minimum.floor() - 1.0;
    maximum = maximum.ceil() + 1.0;
    if maximum - minimum < 4.0 {
        let center = (minimum + maximum) / 2.0;
        minimum = center - 2.0;
        maximum = center + 2.0;
    }
    (minimum, maximum)
}

fn log_ticks(minimum: f64, maximum: f64) -> Vec<f64> {
    (0..TICK_COUNT)
        .map(|index| {
            let fraction = index as f64 / (TICK_COUNT - 1) as f64;
            10.0_f64.powf(minimum + fraction * (maximum - minimum))
        })
        .collect()
}

fn log_fraction(value: f64, minimum: f64, maximum: f64) -> f32 {
    if !value.is_finite() || value <= 0.0 || maximum <= minimum {
        return 0.0;
    }
    ((value.log10() - minimum) / (maximum - minimum)).clamp(0.0, 1.0) as f32
}

fn metric_chip(label: &'static str, value: String) -> Div {
    div()
        .h(px(24.0))
        .flex()
        .items_center()
        .gap_1()
        .rounded_sm()
        .px_2()
        .bg(rgb(CHROME))
        .text_xs()
        .child(div().text_color(rgb(MUTED_TEXT)).child(label))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(value))
}

fn legend_item(color: u32, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().w(px(8.0)).h(px(8.0)).bg(rgb(color)))
        .child(label)
}

fn roofline_loop_table_header() -> Div {
    div()
        .h(px(LOOP_HEADER_HEIGHT))
        .min_h(px(LOOP_HEADER_HEIGHT))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .child(div().min_w(px(0.0)).flex_1().px_2().child("LOOP"))
        .child(header_cell("GFLOP/s", GFLOPS_WIDTH))
        .child(header_cell("AI", AI_WIDTH))
        .child(header_cell("ROOF %", EFFICIENCY_WIDTH))
        .child(div().w(px(22.0)).min_w(px(22.0)))
}

fn header_cell(label: &'static str, width: f32) -> Div {
    div()
        .w(px(width))
        .min_w(px(width))
        .pr_2()
        .text_right()
        .child(label)
}

fn numeric_cell(value: String, width: f32) -> Div {
    div()
        .w(px(width))
        .min_w(px(width))
        .pr_2()
        .text_right()
        .text_xs()
        .child(value)
}

fn loop_location(loop_data: &RooflineLoop) -> String {
    match (loop_data.file_name.trim().is_empty(), loop_data.line > 0) {
        (false, true) => format!("{}:{}", loop_data.file_name, loop_data.line),
        (false, false) => loop_data.file_name.clone(),
        (true, true) => format!("Line {}", loop_data.line),
        (true, false) => "No source location".to_string(),
    }
}

fn axis_tick(value: f64) -> Div {
    div().child(format_axis_value(value))
}

fn format_axis_value(value: f64) -> String {
    if value >= 1_000.0 {
        format!("{:.0}k", value / 1_000.0)
    } else if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else if value >= 1.0 {
        format!("{value:.2}")
    } else if value >= 0.01 {
        format!("{value:.3}")
    } else {
        format!("{value:.1e}")
    }
}

fn format_metric(value: f64) -> String {
    if value >= 100.0 {
        format!("{value:.1}")
    } else if value >= 1.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}

fn roofline_message(message: impl Into<String>, error: bool) -> Div {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_data(name: &str, gflops: f64, intensity: f64) -> RooflineLoop {
        RooflineLoop {
            function_name: name.to_string(),
            file_name: "/src/kernel.c".to_string(),
            line: 1,
            scalar_int_ops: None,
            scalar_int_ai: None,
            scalar_float_ops: None,
            scalar_float_ai: None,
            scalar_double_ops: Some(gflops * 1_000_000_000.0),
            scalar_double_ai: Some(intensity),
            vector_int_ops: None,
            vector_int_ai: None,
            vector_float_ops: None,
            vector_float_ai: None,
            vector_double_ops: None,
            vector_double_ai: None,
        }
    }

    #[test]
    fn plot_keeps_every_positive_fp64_loop_and_maps_logarithmically() {
        let data = RooflineData {
            loops: vec![loop_data("low", 1.0, 0.1), loop_data("high", 100.0, 10.0)],
            calibration: None,
            error: None,
        };
        let plot = RooflinePlot::build(&data);

        assert_eq!(plot.points.len(), 2);
        assert!(plot.x_fraction(0.1) < plot.x_fraction(10.0));
        assert!(plot.y_fraction(1.0) < plot.y_fraction(100.0));
        assert_eq!(plot.x_ticks().len(), TICK_COUNT);
        assert_eq!(plot.y_ticks().len(), TICK_COUNT);
    }

    #[test]
    fn plot_excludes_unplottable_loops_without_dropping_them_from_data() {
        let mut integer_only = loop_data("integer-only", 1.0, 1.0);
        integer_only.scalar_double_ops = Some(0.0);
        integer_only.scalar_double_ai = Some(0.0);
        let data = RooflineData {
            loops: vec![loop_data("fp64", 10.0, 2.0), integer_only],
            calibration: None,
            error: None,
        };

        let plot = RooflinePlot::build(&data);

        assert_eq!(data.loops.len(), 2);
        assert_eq!(plot.points.len(), 1);
        assert_eq!(plot.points[0].loop_index, 0);
    }
}
