use std::path::PathBuf;

use gpui::{
    Context, Div, FontWeight, IntoElement, MouseButton, SharedString, div, prelude::*, px,
    relative, rgb,
};

use crate::{
    BottomPanelKind, MperfGui,
    metrics::{MetricsColumn, MetricsTableData},
    model::{CounterRow, GuiTab, ResultsModel, TmaSummaryData, TmaSummaryRow},
    profile::{CounterMetric, ProfileFrame, TimeRange},
    profile_analysis::{FunctionAnalysis, FunctionStat, SampleFilter},
    source::SourceLocation,
    theme::{
        ACCENT, BORDER, BOTTOM_PANEL_MIN_HEIGHT, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED,
        SURFACE, TEXT, WORKSPACE,
    },
};

const PANEL_HEADER_HEIGHT: f32 = 28.0;
const TABLE_HEADER_HEIGHT: f32 = 24.0;
const TABLE_ROW_HEIGHT: f32 = 24.0;
const HOTSPOTS_TABLE_WIDTH: f32 = 1_230.0;
const SHARE_WIDTH: f32 = 124.0;
const CPU_TIME_WIDTH: f32 = 100.0;
const IPC_WIDTH: f32 = 72.0;
const LLC_MPKI_WIDTH: f32 = 92.0;
const LLC_MISS_WIDTH: f32 = 92.0;
const BACKEND_STALL_WIDTH: f32 = 108.0;
const BRANCH_MPKI_WIDTH: f32 = 102.0;

impl MperfGui {
    pub(crate) fn render_bottom_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_open = !self.bottom_panel_collapsed;
        div()
            .id("bottom-panel-resizer")
            .h(px(if panel_open { 4.0 } else { 0.0 }))
            .min_h(px(if panel_open { 4.0 } else { 0.0 }))
            .w_full()
            .when(panel_open, |element| {
                element
                    .bg(rgb(BORDER))
                    .cursor_row_resize()
                    .hover(|element| element.bg(rgb(ACCENT)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _, cx| {
                            view.bottom_panel_resizing = true;
                            cx.notify();
                        }),
                    )
            })
    }

    pub(crate) fn render_bottom_panel(&self, cx: &mut Context<Self>) -> Div {
        if self.bottom_panel_collapsed {
            return div().h(px(0.0)).min_h(px(0.0));
        }
        let panel = self.active_bottom_panel;

        let content = match panel {
            BottomPanelKind::Statistics => self.render_statistics_panel().into_any_element(),
            BottomPanelKind::Hotspots => self.render_hotspots_panel(cx).into_any_element(),
        };

        div()
            .h(px(self.bottom_panel_height.max(BOTTOM_PANEL_MIN_HEIGHT)))
            .min_h(px(BOTTOM_PANEL_MIN_HEIGHT))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(WORKSPACE))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(self.render_bottom_panel_header(panel, cx))
            .child(
                div()
                    .min_h(px(0.0))
                    .flex_1()
                    .overflow_hidden()
                    .child(content),
            )
    }

    fn render_bottom_panel_header(&self, panel: BottomPanelKind, cx: &mut Context<Self>) -> Div {
        let filter_summary = matches!(panel, BottomPanelKind::Hotspots)
            .then(|| {
                let filter = self.profile_sample_filter();
                self.model.as_ref().and_then(|model| {
                    active_filter_summary(
                        &filter,
                        model.profile.full_range().map(|range| range.start_ns),
                        &model.profile.frames,
                        &model.profile.counter_metrics,
                    )
                })
            })
            .flatten();

        div()
            .h(px(PANEL_HEADER_HEIGHT))
            .min_h(px(PANEL_HEADER_HEIGHT))
            .flex()
            .items_center()
            .bg(rgb(CHROME))
            .border_b_1()
            .border_color(rgb(BORDER))
            .px_2()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .w(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(ACCENT))
                    .child(panel.icon()),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(panel.title()),
            )
            .when(matches!(panel, BottomPanelKind::Hotspots), |element| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child("leaf-attributed PMU metrics · horizontal scroll"),
                )
            })
            .when_some(filter_summary, |element, summary| {
                element
                    .child(
                        div()
                            .h(px(18.0))
                            .max_w(px(520.0))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .rounded_sm()
                            .px_2()
                            .bg(rgb(SURFACE))
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(summary),
                    )
                    .child(
                        div()
                            .id("clear-hotspots-filter")
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .px_2()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.clear_global_filter();
                                cx.notify();
                            }))
                            .child("Clear"),
                    )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("close-bottom-panel")
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(MUTED_TEXT))
                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.bottom_panel_collapsed = true;
                        view.bottom_panel_resizing = false;
                        cx.notify();
                    }))
                    .child("×"),
            )
    }

    fn render_statistics_panel(&self) -> Div {
        let Some(model) = self.model.as_ref() else {
            return empty_message("Open a recording to view statistics.");
        };
        let command = model
            .record_info
            .command
            .as_ref()
            .map(|command| command.join(" "))
            .filter(|command| !command.is_empty())
            .unwrap_or_else(|| "Attached process".to_string());
        let sampling = model
            .record_info
            .sampling_frequency_hz
            .map(|frequency| format!("{frequency} Hz"))
            .unwrap_or_else(|| "Not recorded".to_string());
        let samples = model.profile.samples.len().to_string();
        let format_version = if model.record_info.format_version == 0 {
            "Legacy".to_string()
        } else {
            model.record_info.format_version.to_string()
        };
        let metric_tables = model
            .tabs
            .iter()
            .filter_map(|tab| match tab {
                GuiTab::MetricsTable { title, data } => Some((title.as_str(), data)),
                _ => None,
            })
            .collect::<Vec<_>>();

        let primary_statistics = model.tma_summary.as_ref().map_or_else(
            || render_performance_counter_statistics(model),
            render_tma_statistics,
        );
        let show_recorded_analysis = model.tma_summary.is_none() && !metric_tables.is_empty();
        let content_min_width = if model.tma_summary.is_some() {
            860.0
        } else if show_recorded_analysis {
            1_060.0
        } else {
            680.0
        };

        div().size_full().bg(rgb(WORKSPACE)).child(
            div()
                .id("statistics-horizontal-scroll")
                .size_full()
                .overflow_x_scroll()
                .child(
                    div()
                        .h_full()
                        .min_w(px(content_min_width))
                        .flex()
                        .child(primary_statistics)
                        .when(show_recorded_analysis, |element| {
                            element.child(
                                div()
                                    .min_w(px(380.0))
                                    .min_h(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .child(section_header("Recorded Analysis"))
                                    .child(
                                        div()
                                            .id("statistics-analysis-scroll")
                                            .min_h(px(0.0))
                                            .flex_1()
                                            .overflow_y_scroll()
                                            .children(metric_tables.into_iter().enumerate().map(
                                                |(index, (title, table))| {
                                                    render_static_metrics_table(index, title, table)
                                                },
                                            )),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .w(px(340.0))
                                .min_w(px(280.0))
                                .min_h(px(0.0))
                                .flex()
                                .flex_col()
                                .child(section_header("Recording"))
                                .child(
                                    div()
                                        .id("statistics-recording-scroll")
                                        .min_h(px(0.0))
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .children([
                                            info_row("Scenario", model.record_info.scenario.name()),
                                            info_row("Command", command),
                                            info_row("CPU", model.record_info.cpu_model.clone()),
                                            info_row(
                                                "Vendor",
                                                model.record_info.cpu_vendor.clone(),
                                            ),
                                            info_row("Sampling", sampling),
                                            info_row("Samples", samples),
                                            info_row("Format", format_version),
                                        ]),
                                ),
                        ),
                ),
        )
    }

    fn render_hotspots_panel(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return empty_message("Open a recording to inspect hotspots.");
        };
        if let Some(error) = model.profile.error.as_ref() {
            return error_message(error.clone());
        }
        if model.profile.samples.is_empty() {
            return empty_message("This recording has no timestamped profile samples.");
        }

        let filter = self.profile_sample_filter();
        let selected_frames = filter.frame_ids.clone().unwrap_or_default();
        let analysis = FunctionAnalysis::build(&model.profile, &filter);
        if analysis.total_samples == 0 {
            return empty_message("No profile samples match the active filter.");
        }
        if analysis.functions.is_empty() {
            return empty_message("No functions were resolved for the matching samples.");
        }

        div().size_full().min_h(px(0.0)).bg(rgb(WORKSPACE)).child(
            div()
                .id("hotspots-horizontal-scroll")
                .size_full()
                .overflow_x_scroll()
                .child(
                    div()
                        .w(px(HOTSPOTS_TABLE_WIDTH))
                        .min_w(px(HOTSPOTS_TABLE_WIDTH))
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(hotspots_header())
                        .child(
                            div()
                                .id("hotspots-function-scroll")
                                .min_h(px(0.0))
                                .flex_1()
                                .overflow_y_scroll()
                                .children(analysis.functions.iter().enumerate().map(
                                    |(row_index, function)| {
                                        self.render_hotspot_row(
                                            row_index,
                                            function,
                                            selected_frames.contains(&function.frame_id),
                                            cx,
                                        )
                                    },
                                )),
                        ),
                ),
        )
    }

    fn render_hotspot_row(
        &self,
        row_index: usize,
        function: &FunctionStat,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let frame_id = function.frame_id;
        let label = function.label.clone();
        let self_share = format_share(function.self_fraction, function.self_samples);
        let inclusive_share = format_share(function.inclusive_fraction, function.inclusive_samples);
        let cpu_time = format_cpu_time(function.metrics.cpu_time_ns);
        let ipc = format_metric(function.metrics.ipc, 2);
        let llc_mpki = format_metric(function.metrics.llc_mpki, 2);
        let llc_miss = format_optional_percent(function.metrics.llc_miss_rate);
        let backend_stall = format_optional_percent(function.metrics.backend_stall_fraction);
        let branch_mpki = format_metric(function.metrics.branch_mpki, 2);

        div()
            .id(SharedString::from(format!("hotspot-row-{row_index}")))
            .h(px(TABLE_ROW_HEIGHT))
            .min_h(px(TABLE_ROW_HEIGHT))
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
                view.select_profile_function(frame_id);
                if event.click_count() >= 2 {
                    view.open_profile_frame_source(frame_id);
                }
                cx.notify();
            }))
            .child(
                div()
                    .min_w(px(240.0))
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .px_3()
                    .child(label),
            )
            .child(numeric_cell(self_share, SHARE_WIDTH))
            .child(numeric_cell(inclusive_share, SHARE_WIDTH))
            .child(numeric_cell(cpu_time, CPU_TIME_WIDTH))
            .child(numeric_cell(ipc, IPC_WIDTH))
            .child(numeric_cell(llc_mpki, LLC_MPKI_WIDTH))
            .child(numeric_cell(llc_miss, LLC_MISS_WIDTH))
            .child(numeric_cell(backend_stall, BACKEND_STALL_WIDTH))
            .child(numeric_cell(branch_mpki, BRANCH_MPKI_WIDTH))
    }

    fn open_profile_frame_source(&mut self, frame_id: usize) {
        let location = self
            .model
            .as_ref()
            .and_then(|model| model.profile.frames.get(frame_id))
            .and_then(|frame| {
                Some(SourceLocation {
                    path: PathBuf::from(frame.file.as_ref()?),
                    line: frame.line? as usize,
                })
            });

        if let Some(location) = location {
            self.open_source(location);
        } else {
            self.load_error =
                Some("No source location was recorded for this function.".to_string());
        }
    }
}

fn active_filter_summary(
    filter: &SampleFilter,
    profile_start_ns: Option<u64>,
    frames: &[ProfileFrame],
    counters: &[CounterMetric],
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(range) = filter.range {
        parts.push(match profile_start_ns {
            Some(profile_start_ns) => format_relative_time_range(range, profile_start_ns),
            None => "time range".to_string(),
        });
    }
    if let Some(process_id) = filter.process_id {
        parts.push(format!("PID {process_id}"));
    }
    if let Some(thread_id) = filter.thread_id {
        parts.push(format!("TID {thread_id}"));
    }
    if let Some(cpu) = filter.cpu {
        parts.push(format!("CPU {cpu}"));
    }
    if let Some(counter_index) = filter.required_counter {
        let counter = counters
            .get(counter_index)
            .map(|counter| counter.label.as_str())
            .unwrap_or("counter");
        parts.push(format!("{counter} present"));
    }
    if let Some(frame_ids) = filter.frame_ids.as_ref() {
        let functions = frame_ids
            .iter()
            .filter_map(|frame_id| frames.get(*frame_id))
            .map(|frame| frame.name.as_str())
            .take(2)
            .collect::<Vec<_>>();
        parts.push(match functions.as_slice() {
            [] => "function filter".to_string(),
            [function] => format!("function {function}"),
            [first, second] if frame_ids.len() == 2 => format!("functions {first}, {second}"),
            [first, second] => {
                format!(
                    "functions {first}, {second} +{}",
                    frame_ids.len().saturating_sub(2)
                )
            }
            _ => unreachable!(),
        });
    }

    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

fn format_relative_time_range(range: TimeRange, profile_start_ns: u64) -> String {
    let start_ns = range.start_ns.saturating_sub(profile_start_ns);
    let end_ns = range.end_ns.saturating_sub(profile_start_ns);
    format!(
        "time {:.3}–{:.3}s",
        start_ns as f64 / 1_000_000_000.0,
        end_ns as f64 / 1_000_000_000.0
    )
}

fn section_header(title: &'static str) -> Div {
    div()
        .h(px(TABLE_HEADER_HEIGHT))
        .min_h(px(TABLE_HEADER_HEIGHT))
        .flex()
        .items_center()
        .px_3()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title)
}

fn render_counter_row(row: CounterRow) -> Div {
    div()
        .h(px(31.0))
        .min_h(px(31.0))
        .flex()
        .items_center()
        .px_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_sm()
        .hover(|element| element.bg(rgb(HOVER)))
        .child(div().flex_1().child(row.label))
        .child(div().w(px(150.0)).text_right().child(row.value))
        .child(
            div()
                .w(px(110.0))
                .text_right()
                .text_color(rgb(MUTED_TEXT))
                .child(row.detail),
        )
}

fn render_performance_counter_statistics(model: &ResultsModel) -> Div {
    div()
        .min_w(px(340.0))
        .min_h(px(0.0))
        .flex_1()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(BORDER))
        .child(section_header("Performance Counters"))
        .child(
            div()
                .id("statistics-counters-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_y_scroll()
                .children(model.summary.rows().into_iter().map(render_counter_row)),
        )
}

fn render_tma_statistics(summary: &TmaSummaryData) -> Div {
    div()
        .min_w(px(520.0))
        .min_h(px(0.0))
        .flex_1()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(BORDER))
        .child(section_header("Top-Down Metrics · Levels 1–3"))
        .when_some(summary.error.clone(), |element, error| {
            element.child(
                div()
                    .min_h(px(32.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .text_xs()
                    .text_color(rgb(ERROR))
                    .child(format!(
                        "Overall TMA values were not preserved in this legacy recording. \
                         Re-record with the current profiler. {error}"
                    )),
            )
        })
        .child(
            div()
                .id("statistics-tma-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_y_scroll()
                .when(summary.rows.is_empty(), |element| {
                    element.child(
                        div()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p_4()
                            .text_sm()
                            .text_color(rgb(MUTED_TEXT))
                            .child("This TMA scenario defines no Level 1–3 metrics."),
                    )
                })
                .children(summary.rows.iter().map(render_tma_row)),
        )
}

fn render_tma_row(row: &TmaSummaryRow) -> Div {
    let value = row
        .value
        .map(|value| format!("{:.2}%", value * 100.0))
        .unwrap_or_else(|| "N/A".to_string());
    let fraction = row.value.unwrap_or_default().clamp(0.0, 1.0) as f32;
    let color = tma_metric_color(&row.name);
    let indent = row.level.saturating_sub(1) as f32 * 18.0;

    div()
        .min_h(px(34.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .hover(|element| element.bg(rgb(HOVER)))
        .child(
            div()
                .w(px(44.0))
                .min_w(px(44.0))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(MUTED_TEXT))
                .child(format!("L{}", row.level)),
        )
        .child(
            div()
                .min_w(px(250.0))
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .pl(px(12.0 + indent))
                .pr_2()
                .gap_2()
                .child(div().w(px(4.0)).h(px(20.0)).rounded_sm().bg(rgb(color)))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tma_metric_label(&row.name)),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(MUTED_TEXT))
                                .child(row.description.clone()),
                        ),
                )
                .when(row.dominant, |element| {
                    element.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child("dominant"),
                    )
                }),
        )
        .child(
            div()
                .w(px(160.0))
                .min_w(px(160.0))
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .border_l_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .w(px(96.0))
                        .h(px(7.0))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(rgb(SURFACE))
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_sm()
                                .bg(rgb(color)),
                        ),
                )
                .child(
                    div()
                        .w(px(58.0))
                        .text_right()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value),
                ),
        )
}

fn tma_metric_label(name: &str) -> String {
    match name {
        "retiring" => "Retiring".to_string(),
        "fe_bound" => "Frontend Bound".to_string(),
        "bad_speculation" => "Bad Speculation".to_string(),
        "be_bound" => "Backend Bound".to_string(),
        _ => {
            let leaf = name.rsplit('.').next().unwrap_or(name);
            leaf.split('_')
                .filter(|word| !word.is_empty())
                .map(|word| {
                    let mut chars = word.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

fn tma_metric_color(name: &str) -> u32 {
    match name.split('.').next().unwrap_or(name) {
        "retiring" => 0x5f966e,
        "fe_bound" => 0xc59654,
        "bad_speculation" => 0xc26167,
        "be_bound" => 0x6d86b3,
        _ => ACCENT,
    }
}

fn info_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .min_h(px(30.0))
        .flex()
        .items_start()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_sm()
        .hover(|element| element.bg(rgb(HOVER)))
        .child(
            div()
                .w(px(82.0))
                .text_color(rgb(MUTED_TEXT))
                .child(label.into()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(value.into()),
        )
}

fn render_static_metrics_table(index: usize, title: &str, table: &MetricsTableData) -> Div {
    let content_height =
        ((table.rows.len().min(8) + 1) as f32 * TABLE_ROW_HEIGHT).clamp(90.0, 270.0);
    let table_width = table.total_width().max(360.0);

    div()
        .min_h(px(content_height + TABLE_HEADER_HEIGHT))
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .h(px(TABLE_HEADER_HEIGHT))
                .min_h(px(TABLE_HEADER_HEIGHT))
                .flex()
                .items_center()
                .px_3()
                .bg(rgb(SURFACE))
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .when_some(table.error.clone(), |element, error| {
            element.child(
                div()
                    .h(px(content_height))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_3()
                    .text_xs()
                    .text_color(rgb(ERROR))
                    .child(error),
            )
        })
        .when(table.error.is_none(), |element| {
            element.child(
                div()
                    .id(SharedString::from(format!(
                        "statistics-analysis-table-{index}"
                    )))
                    .h(px(content_height))
                    .overflow_scroll()
                    .child(
                        div()
                            .w(px(table_width))
                            .min_w(px(table_width))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(TABLE_ROW_HEIGHT))
                                    .min_h(px(TABLE_ROW_HEIGHT))
                                    .flex()
                                    .bg(rgb(CHROME))
                                    .border_b_1()
                                    .border_color(rgb(BORDER))
                                    .children(
                                        table.columns.iter().map(|column| {
                                            static_metric_cell(column, &column.label)
                                        }),
                                    ),
                            )
                            .children(table.rows.iter().map(|row| {
                                div()
                                    .h(px(TABLE_ROW_HEIGHT))
                                    .min_h(px(TABLE_ROW_HEIGHT))
                                    .flex()
                                    .border_b_1()
                                    .border_color(rgb(BORDER))
                                    .hover(|element| element.bg(rgb(HOVER)))
                                    .children(
                                        table.columns.iter().zip(row.values.iter()).map(
                                            |(column, value)| static_metric_cell(column, value),
                                        ),
                                    )
                            })),
                    ),
            )
        })
}

fn static_metric_cell(column: &MetricsColumn, value: &str) -> Div {
    div()
        .w(px(column.width))
        .min_w(px(column.width))
        .h_full()
        .flex()
        .items_center()
        .px_2()
        .border_r_1()
        .border_color(rgb(BORDER))
        .overflow_hidden()
        .whitespace_nowrap()
        .when(column.align_right, |element| {
            element.justify_end().text_right()
        })
        .child(value.to_string())
}

fn hotspots_header() -> Div {
    div()
        .h(px(TABLE_HEADER_HEIGHT))
        .min_h(px(TABLE_HEADER_HEIGHT))
        .flex()
        .items_center()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED_TEXT))
        .child(
            div()
                .min_w(px(240.0))
                .min_w_0()
                .flex_1()
                .px_3()
                .child("Function"),
        )
        .child(header_cell("Self % · samples", SHARE_WIDTH))
        .child(header_cell("Total % · samples", SHARE_WIDTH))
        .child(header_cell("CPU time", CPU_TIME_WIDTH))
        .child(header_cell("IPC", IPC_WIDTH))
        .child(header_cell("LLC MPKI", LLC_MPKI_WIDTH))
        .child(header_cell("LLC miss", LLC_MISS_WIDTH))
        .child(header_cell("Backend stall", BACKEND_STALL_WIDTH))
        .child(header_cell("Branch MPKI", BRANCH_MPKI_WIDTH))
}

fn header_cell(label: &'static str, width: f32) -> Div {
    div()
        .w(px(width))
        .min_w(px(width))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .px_3()
        .border_l_1()
        .border_color(rgb(BORDER))
        .child(label)
}

fn numeric_cell(value: String, width: f32) -> Div {
    div()
        .w(px(width))
        .min_w(px(width))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .px_3()
        .border_l_1()
        .border_color(rgb(BORDER))
        .text_right()
        .child(value)
}

fn format_percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

fn format_share(fraction: f64, samples: u64) -> String {
    format!("{} · {samples}", format_percent(fraction))
}

fn format_metric(value: Option<f64>, precision: usize) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.precision$}"))
        .unwrap_or_else(|| "—".to_string())
}

fn format_optional_percent(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(format_percent)
        .unwrap_or_else(|| "—".to_string())
}

fn format_cpu_time(value: Option<f64>) -> String {
    let Some(nanoseconds) = value.filter(|value| value.is_finite() && *value >= 0.0) else {
        return "—".to_string();
    };
    if nanoseconds >= 1_000_000_000.0 {
        format!("{:.2} s", nanoseconds / 1_000_000_000.0)
    } else if nanoseconds >= 1_000_000.0 {
        format!("{:.2} ms", nanoseconds / 1_000_000.0)
    } else if nanoseconds >= 1_000.0 {
        format!("{:.1} µs", nanoseconds / 1_000.0)
    } else {
        format!("{nanoseconds:.0} ns")
    }
}

fn empty_message(message: &'static str) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(rgb(MUTED_TEXT))
        .bg(rgb(SURFACE))
        .child(message)
}

fn error_message(message: String) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(rgb(ERROR))
        .bg(rgb(SURFACE))
        .child(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_time_filter_is_relative_to_profile_start() {
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 11_250_000_000,
                end_ns: 12_500_000_000,
            }),
            ..SampleFilter::default()
        };

        assert_eq!(
            active_filter_summary(&filter, Some(10_000_000_000), &[], &[]).as_deref(),
            Some("time 1.250–2.500s")
        );
    }
}
