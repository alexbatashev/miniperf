use gpui::{
    Bounds, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder,
    SharedString, Transformation, canvas, div, fill, point, prelude::*, px, radians, relative, rgb,
    size, svg,
};

use crate::{
    MperfGui,
    roofline::{roofline_label_asset, RooflineData, RooflineLoop},
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED, SURFACE, TEXT, WORKSPACE,
    },
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
const ROOF_LABEL_WIDTH: f32 = 176.0;
const ROOF_LABEL_HEIGHT: f32 = 18.0;

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
            .child(div().font_weight(FontWeight::SEMIBOLD).child(
                if data.uses_architectural_traffic() && data.has_compatible_memory_roof() {
                    "FP64 Cache-Aware Roofline"
                } else if data.has_compatible_memory_roof() {
                    if data.uses_modeled_traffic() {
                        "FP64 Roofline · modeled LLC traffic"
                    } else {
                        "FP64 Roofline"
                    }
                } else {
                    "FP64 Architectural-Intensity Analysis"
                },
            ))
            .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(format!(
                "{plotted_count} plotted · {loop_count} recorded loops"
            )))
            .when_some(data.method.as_ref(), |element, method| {
                element.child(metric_chip(
                    "Method",
                    format!("{} · {}", method.performance, method.quality),
                ))
            })
            .child(div().flex_1())
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
        let plot_labels = plot
            .labels(self.roofline_chart_size)
            .into_iter()
            .enumerate()
            .filter(|(index, _)| {
                self.roofline_labels_always_visible || self.hovered_roofline_roof == Some(*index)
            })
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        let plot_for_paint = plot.clone();
        let plot_for_click = plot.clone();
        let entity = cx.entity();
        let graph = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_roofline(bounds, &plot_for_paint, selected, window);

                let click_plot = plot_for_click.clone();
                let click_entity = entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
                    if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                        return;
                    }
                    let Some(index) = nearest_point(bounds, &click_plot, event.position) else {
                        return;
                    };
                    click_entity.update(cx, |view, cx| {
                        view.select_roofline_loop(index, event.click_count >= 2);
                        cx.notify();
                    });
                });

                let hover_plot = plot_for_click.clone();
                let hover_entity = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    let hovered = bounds
                        .contains(&event.position)
                        .then(|| nearest_roof(bounds, &hover_plot, event.position))
                        .flatten();
                    let chart_size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    hover_entity.update(cx, |view, cx| {
                        if view.hovered_roofline_roof != hovered
                            || view.roofline_chart_size != Some(chart_size)
                        {
                            view.hovered_roofline_roof = hovered;
                            view.roofline_chart_size = Some(chart_size);
                            cx.notify();
                        }
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
                    .child(legend_item(
                        ACCENT,
                        if data.uses_architectural_traffic() && data.has_compatible_memory_roof() {
                            "Calibrated cache hierarchy roofs"
                        } else if data.has_compatible_memory_roof() {
                            "Calibrated attainable roof"
                        } else {
                            "Calibrated FP64 compute ceiling"
                        },
                    ))
                    .child(legend_item(POINT, "Recorded FP64 loop"))
                    .child(div().flex_1())
                    .child(roofline_label_toggle(
                        self.roofline_labels_always_visible,
                        cx,
                    ))
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
                                    .child(graph)
                                    .children(plot_labels.into_iter().map(|label| {
                                        svg()
                                            .absolute()
                                            .left(relative(label.x_fraction))
                                            .top(relative(1.0 - label.y_fraction))
                                            .ml(px(-ROOF_LABEL_WIDTH / 2.0))
                                            .mt(px(-ROOF_LABEL_HEIGHT))
                                            .w(px(ROOF_LABEL_WIDTH))
                                            .h(px(ROOF_LABEL_HEIGHT))
                                            .path(roofline_label_asset(&label.text))
                                            .text_color(rgb(label.color))
                                            .with_transformation(Transformation::rotate(radians(
                                                label.rotation_radians,
                                            )))
                                    })),
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
                                    .child(if data.uses_architectural_traffic() {
                                        "Architectural intensity (FLOP / architectural byte)"
                                    } else if data.has_compatible_memory_roof() {
                                        if data.uses_modeled_traffic() {
                                            "Modeled DRAM intensity (FLOP / byte)"
                                        } else {
                                            "Arithmetic intensity (FLOP / byte)"
                                        }
                                    } else {
                                        "Architectural intensity (FLOP / architectural byte)"
                                    }),
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
    ) -> impl IntoElement + use<> {
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
            .efficiency(loop_data)
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
    /// One bandwidth roof per memory hierarchy level, fastest first. Loops are
    /// plotted against architectural traffic (cache-aware roofline), so a
    /// cache-resident loop sits above the DRAM roof and is bounded by whichever
    /// level actually serves it.
    roofs: Vec<(String, f64)>,
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

#[derive(Debug, PartialEq)]
struct RooflineLabel {
    text: String,
    x_fraction: f32,
    y_fraction: f32,
    color: u32,
    rotation_radians: f32,
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
        if data.has_compatible_memory_roof() {
            let calibration = data.calibration.as_ref().expect("checked above");
            x_values.push(calibration.ridge_point_flops_per_byte);
            y_values.push(calibration.fp64_gflops);
        } else if let Some(calibration) = data.calibration.as_ref() {
            y_values.push(calibration.fp64_gflops);
        }

        let roofs = data
            .bandwidth_roofs()
            .into_iter()
            .map(|(level, bandwidth)| (level.to_string(), bandwidth))
            .collect::<Vec<_>>();
        // Every roof's ridge point has to be inside the x extent, otherwise the
        // fastest level's knee is clipped off the chart.
        if let Some(calibration) = data.calibration.as_ref() {
            for (_, bandwidth) in &roofs {
                let ridge = calibration.fp64_gflops / bandwidth;
                if ridge.is_finite() && ridge > 0.0 {
                    x_values.push(ridge);
                }
            }
        }

        let (x_min_log, x_max_log) = log_extent(&x_values, -2.0, 2.0);
        for (_, bandwidth) in &roofs {
            let value = bandwidth * 10.0_f64.powf(x_min_log);
            if value.is_finite() && value > 0.0 {
                y_values.push(value);
            }
        }
        let (y_min_log, y_max_log) = log_extent(&y_values, -1.0, 3.0);

        Self {
            points,
            calibration: data.calibration.clone(),
            roofs,
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

    fn labels(&self, chart_size: Option<(f32, f32)>) -> Vec<RooflineLabel> {
        let Some(calibration) = self.calibration.as_ref() else {
            return Vec::new();
        };

        let (chart_width, chart_height) = chart_size.unwrap_or((1.0, 1.0));
        let roof_slope = ((chart_height / chart_width)
            * ((self.x_max_log - self.x_min_log) / (self.y_max_log - self.y_min_log)) as f32)
            .atan();
        let mut labels = self
            .roofs
            .iter()
            .enumerate()
            .map(|(index, (level, bandwidth))| {
                // Stagger labels along the parallel sloped roofs. Keeping each
                // one below its ridge places the value directly on its line
                // without stacking the close L2/L3 labels on top of each other.
                let desired_fraction = 0.14 + index as f64 * 0.14;
                let desired_log =
                    self.x_min_log + desired_fraction * (self.x_max_log - self.x_min_log);
                let ridge_log = (calibration.fp64_gflops / bandwidth).log10();
                let intensity = 10.0_f64.powf(desired_log.min(ridge_log - 0.08));
                let gflops = bandwidth * intensity;
                RooflineLabel {
                    text: format!("{level} · {bandwidth:.2} GB/s"),
                    x_fraction: self.x_fraction(intensity),
                    y_fraction: self.y_fraction(gflops),
                    color: if index == 0 { ACCENT } else { MUTED_TEXT },
                    rotation_radians: -roof_slope,
                }
            })
            .collect::<Vec<_>>();

        labels.push(RooflineLabel {
            text: format!("FP64 · {:.2} GFLOP/s", calibration.fp64_gflops),
            x_fraction: 0.72,
            y_fraction: self.y_fraction(calibration.fp64_gflops),
            color: ACCENT,
            rotation_radians: 0.0,
        });
        labels
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
        // Without usable bandwidth roofs only the compute ceiling is known.
        if plot.roofs.is_empty() {
            let mut path = PathBuilder::stroke(px(2.0));
            path.move_to(plot_position(
                bounds,
                plot,
                10.0_f64.powf(plot.x_min_log),
                calibration.fp64_gflops,
            ));
            path.line_to(plot_position(
                bounds,
                plot,
                10.0_f64.powf(plot.x_max_log),
                calibration.fp64_gflops,
            ));
            if let Ok(path) = path.build() {
                window.paint_path(path, rgb(ACCENT));
            }
        } else {
            // Draw the slowest level first so the fastest, which is the roof a
            // cache-resident loop is actually bounded by, ends up on top.
            for (index, (_, bandwidth)) in plot.roofs.iter().enumerate().rev() {
                let binding = index == 0;
                let mut path = PathBuilder::stroke(px(if binding { 2.0 } else { 1.0 }));
                for (point_index, (intensity, gflops)) in
                    roof_path_points(plot, calibration.fp64_gflops, *bandwidth)
                        .into_iter()
                        .enumerate()
                {
                    let position = plot_position(bounds, plot, intensity, gflops);
                    if point_index == 0 {
                        path.move_to(position);
                    } else {
                        path.line_to(position);
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(if binding { ACCENT } else { MUTED_TEXT }));
                }
            }
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

fn roof_path_points(
    plot: &RooflinePlot,
    compute_gflops: f64,
    bandwidth_gbytes_per_second: f64,
) -> Vec<(f64, f64)> {
    let x_min = 10.0_f64.powf(plot.x_min_log);
    let x_max = 10.0_f64.powf(plot.x_max_log);
    let ridge = compute_gflops / bandwidth_gbytes_per_second;
    let mut points = vec![(
        x_min,
        compute_gflops.min(bandwidth_gbytes_per_second * x_min),
    )];
    if ridge > x_min && ridge < x_max {
        points.push((ridge, compute_gflops));
    }
    points.push((
        x_max,
        compute_gflops.min(bandwidth_gbytes_per_second * x_max),
    ));
    points
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

fn nearest_roof(
    bounds: Bounds<gpui::Pixels>,
    plot: &RooflinePlot,
    position: gpui::Point<gpui::Pixels>,
) -> Option<usize> {
    let calibration = plot.calibration.as_ref()?;
    let x_min = 10.0_f64.powf(plot.x_min_log);
    let x_max = 10.0_f64.powf(plot.x_max_log);
    let mut candidates = plot
        .roofs
        .iter()
        .enumerate()
        .filter_map(|(index, (_, bandwidth))| {
            let ridge = calibration.fp64_gflops / bandwidth;
            (ridge > x_min).then(|| {
                let start = plot_position(bounds, plot, x_min, bandwidth * x_min);
                let end_intensity = ridge.min(x_max);
                let end = plot_position(bounds, plot, end_intensity, bandwidth * end_intensity);
                (index, point_segment_distance_squared(position, start, end))
            })
        })
        .collect::<Vec<_>>();

    let compute_start = plot
        .roofs
        .iter()
        .map(|(_, bandwidth)| calibration.fp64_gflops / bandwidth)
        .filter(|ridge| ridge.is_finite() && *ridge > 0.0)
        .min_by(f64::total_cmp)
        .unwrap_or(x_min)
        .clamp(x_min, x_max);
    candidates.push((
        plot.roofs.len(),
        point_segment_distance_squared(
            position,
            plot_position(bounds, plot, compute_start, calibration.fp64_gflops),
            plot_position(bounds, plot, x_max, calibration.fp64_gflops),
        ),
    ));

    candidates
        .into_iter()
        .filter(|(_, distance_squared)| *distance_squared <= 10.0 * 10.0)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn point_segment_distance_squared(
    point: gpui::Point<gpui::Pixels>,
    start: gpui::Point<gpui::Pixels>,
    end: gpui::Point<gpui::Pixels>,
) -> f32 {
    let segment_x = f32::from(end.x - start.x);
    let segment_y = f32::from(end.y - start.y);
    let point_x = f32::from(point.x - start.x);
    let point_y = f32::from(point.y - start.y);
    let length_squared = segment_x * segment_x + segment_y * segment_y;
    if length_squared <= f32::EPSILON {
        return point_x * point_x + point_y * point_y;
    }
    let fraction = ((point_x * segment_x + point_y * segment_y) / length_squared).clamp(0.0, 1.0);
    let dx = point_x - fraction * segment_x;
    let dy = point_y - fraction * segment_y;
    dx * dx + dy * dy
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
    minimum -= 0.35;
    maximum += 0.35;
    if maximum - minimum < 3.0 {
        let center = (minimum + maximum) / 2.0;
        minimum = center - 1.5;
        maximum = center + 1.5;
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

fn roofline_label_toggle(selected: bool, cx: &mut Context<MperfGui>) -> impl IntoElement {
    div()
        .id("roofline-show-labels")
        .h(px(20.0))
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_color(rgb(if selected { TEXT } else { MUTED_TEXT }))
        .on_click(cx.listener(|view, _, _, cx| {
            view.roofline_labels_always_visible = !view.roofline_labels_always_visible;
            cx.notify();
        }))
        .child(
            div()
                .w(px(24.0))
                .h(px(14.0))
                .flex()
                .items_center()
                .rounded_full()
                .px(px(2.0))
                .bg(rgb(if selected { ACCENT } else { BORDER }))
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded_full()
                        .bg(rgb(if selected { WORKSPACE } else { MUTED_TEXT }))
                        .when(selected, |element| element.ml(px(10.0))),
                ),
        )
        .child("Show labels")
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
    let location = match (loop_data.file_name.trim().is_empty(), loop_data.line > 0) {
        (false, true) => format!("{}:{}", loop_data.file_name, loop_data.line),
        (false, false) => loop_data.file_name.clone(),
        (true, true) => format!("Line {}", loop_data.line),
        (true, false) => loop_data
            .module_offset
            .as_ref()
            .map(|offset| format!("Binary {offset}"))
            .unwrap_or_else(|| "No source location".to_string()),
    };
    if let Some(quality) = &loop_data.timing_quality {
        let quality = match quality.as_str() {
            "advisor-grade" => "Advisor-grade",
            "low-confidence" => "Low confidence",
            "insufficient-samples" => "Insufficient samples",
            "unclassified-instructions" => "Unclassified instructions",
            other => other,
        };
        let samples = loop_data
            .timing_samples
            .map(|samples| format!(" · {samples} samples"))
            .unwrap_or_default();
        let error = loop_data
            .timing_relative_error
            .map(|error| format!(" · ±{:.1}%", error * 100.0))
            .unwrap_or_default();
        format!("{location} · {quality}{samples}{error}")
    } else {
        location
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
    use mperf_data::{MemoryLevelCalibration, RooflineCalibration, RooflineMethodInfo};

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
            timing_samples: None,
            timing_relative_error: None,
            timing_quality: None,
            module_offset: None,
            trip_count: None,
        }
    }

    fn carm_data() -> RooflineData {
        RooflineData {
            loops: vec![loop_data("matmul", 19.29, 0.083)],
            calibration: Some(RooflineCalibration {
                threads: 8,
                cpu_affinity: Some("0-7".to_string()),
                samples: 5,
                compute_kernel: "x86-avx512-fma-f64".to_string(),
                fp64_gflops: 234.22,
                fp64_gflops_samples: vec![234.22],
                memory_gbytes_per_second: 31.71,
                memory_gbytes_per_second_samples: vec![31.71],
                ridge_point_flops_per_byte: 7.39,
                memory_working_set_bytes: 201_326_592,
                memory_levels: vec![
                    MemoryLevelCalibration {
                        level: "L1".to_string(),
                        gbytes_per_second: 773.43,
                        gbytes_per_second_samples: vec![773.43],
                        working_set_bytes: 98_304,
                    },
                    MemoryLevelCalibration {
                        level: "L2".to_string(),
                        gbytes_per_second: 461.74,
                        gbytes_per_second_samples: vec![461.74],
                        working_set_bytes: 2_621_376,
                    },
                    MemoryLevelCalibration {
                        level: "L3".to_string(),
                        gbytes_per_second: 418.34,
                        gbytes_per_second_samples: vec![418.34],
                        working_set_bytes: 4_194_240,
                    },
                    MemoryLevelCalibration {
                        level: "DRAM".to_string(),
                        gbytes_per_second: 31.71,
                        gbytes_per_second_samples: vec![31.71],
                        working_set_bytes: 201_326_592,
                    },
                ],
            }),
            method: Some(RooflineMethodInfo {
                selection: "auto".to_string(),
                accounting: "dynamorio".to_string(),
                performance: "native".to_string(),
                traffic: "architectural".to_string(),
                quality: "hybrid-binary-aggregate-blocks".to_string(),
                reason: "test".to_string(),
                warnings: Vec::new(),
            }),
            error: None,
        }
    }

    #[test]
    fn carm_plot_contains_every_calibrated_hierarchy_roof() {
        let plot = RooflinePlot::build(&carm_data());

        assert_eq!(
            plot.roofs
                .iter()
                .map(|(level, _)| level.as_str())
                .collect::<Vec<_>>(),
            vec!["L1", "L2", "L3", "DRAM"]
        );
        for (_, bandwidth) in &plot.roofs {
            let ridge = plot.calibration.as_ref().unwrap().fp64_gflops / bandwidth;
            assert!(plot.x_fraction(ridge) > 0.0);
            assert!(plot.x_fraction(ridge) < 1.0);
        }
    }

    #[test]
    fn roof_path_uses_the_exact_ridge_without_an_interpolated_kink() {
        let plot = RooflinePlot::build(&carm_data());
        let calibration = plot.calibration.as_ref().unwrap();
        let bandwidth = plot.roofs[0].1;
        let points = roof_path_points(&plot, calibration.fp64_gflops, bandwidth);

        assert_eq!(points.len(), 3);
        assert_eq!(
            points[1],
            (calibration.fp64_gflops / bandwidth, calibration.fp64_gflops)
        );
        assert_eq!(points[2].1, calibration.fp64_gflops);
    }

    #[test]
    fn carm_labels_follow_the_roof_slope_and_keep_compute_horizontal() {
        let labels = RooflinePlot::build(&carm_data()).labels(Some((600.0, 500.0)));

        assert_eq!(labels.len(), 5);
        assert_eq!(labels[0].text, "L1 · 773.43 GB/s");
        assert!(labels[..4].iter().all(|label| label.rotation_radians < 0.0));
        assert_eq!(labels[4].text, "FP64 · 234.22 GFLOP/s");
        assert_eq!(labels[4].rotation_radians, 0.0);
    }

    #[test]
    fn roof_hover_targets_bandwidth_and_compute_segments_separately() {
        let plot = RooflinePlot::build(&carm_data());
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(600.0), px(500.0)));
        let calibration = plot.calibration.as_ref().unwrap();
        let l1_bandwidth = plot.roofs[0].1;
        let l1_intensity = calibration.fp64_gflops / l1_bandwidth / 2.0;
        let l1_position = plot_position(bounds, &plot, l1_intensity, l1_bandwidth * l1_intensity);
        let compute_position = plot_position(bounds, &plot, 4.0, calibration.fp64_gflops);

        assert_eq!(nearest_roof(bounds, &plot, l1_position), Some(0));
        assert_eq!(
            nearest_roof(bounds, &plot, compute_position),
            Some(plot.roofs.len())
        );
    }

    #[test]
    fn plot_keeps_every_positive_fp64_loop_and_maps_logarithmically() {
        let data = RooflineData {
            loops: vec![loop_data("low", 1.0, 0.1), loop_data("high", 100.0, 10.0)],
            calibration: None,
            method: None,
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
            method: None,
            error: None,
        };

        let plot = RooflinePlot::build(&data);

        assert_eq!(data.loops.len(), 2);
        assert_eq!(plot.points.len(), 1);
        assert_eq!(plot.points[0].loop_index, 0);
    }
}
