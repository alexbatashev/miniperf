mod flamegraph;
mod metrics;
mod model;
mod recent;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use flamelens::flame::ROOT_ID;
use gpui::{
    div, point, prelude::*, px, relative, rgb, size, App, Application, Bounds, Context, Div,
    FontWeight, IntoElement, MouseButton, MouseMoveEvent, PathPromptOptions, ScrollHandle,
    SharedString, Window, WindowBounds, WindowOptions,
};
use num_format::ToFormattedString;

use flamegraph::FlamegraphData;
use metrics::{MetricsColumn, MetricsTableData};
use model::{CounterRow, GuiTab, ResultsModel};

const WORKSPACE: u32 = 0x1e1e1e;
const CHROME: u32 = 0x181818;
const ACTIVE_TAB: u32 = 0x202020;
const HOVER: u32 = 0x252525;
const BORDER: u32 = 0x2a2a2a;
const TEXT: u32 = 0xd4d4d4;
const MUTED_TEXT: u32 = 0x858585;
const ACCENT: u32 = 0x6c8ac4;
const SIDEBAR_DEFAULT_WIDTH: f32 = 230.0;
const SIDEBAR_MIN_WIDTH: f32 = 170.0;
const SIDEBAR_MAX_WIDTH: f32 = 520.0;
const SIDEBAR_COLLAPSED_WIDTH: f32 = 34.0;
const INSPECTOR_DEFAULT_WIDTH: f32 = 330.0;
const INSPECTOR_MIN_WIDTH: f32 = 240.0;
const INSPECTOR_MAX_WIDTH: f32 = 520.0;
const INSPECTOR_COLLAPSED_WIDTH: f32 = 34.0;
const FLAME_FRAME_HEIGHT: f32 = 25.0;

#[derive(Parser)]
#[command(about = "GPU-accelerated viewer for mperf result directories")]
struct Cli {
    /// Directory containing info.json and perf.db.
    result_directory: Option<PathBuf>,
}

struct MperfGui {
    model: Option<ResultsModel>,
    recent_results: Vec<PathBuf>,
    selected_tab: usize,
    picking_directory: bool,
    load_error: Option<String>,
    sidebar_width: f32,
    sidebar_collapsed: bool,
    sidebar_resizing: bool,
    inspector_width: f32,
    inspector_collapsed: bool,
    inspector_resizing: bool,
    flamegraph_instructions: bool,
    flamegraph_zoom: usize,
    metrics_scroll_handle: ScrollHandle,
    metrics_vertical_scroll_handle: ScrollHandle,
}

impl MperfGui {
    fn new(model: Option<ResultsModel>, recent_results: Vec<PathBuf>) -> Self {
        Self {
            model,
            recent_results,
            selected_tab: 0,
            picking_directory: false,
            load_error: None,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            sidebar_collapsed: false,
            sidebar_resizing: false,
            inspector_width: INSPECTOR_DEFAULT_WIDTH,
            inspector_collapsed: false,
            inspector_resizing: false,
            flamegraph_instructions: false,
            flamegraph_zoom: ROOT_ID,
            metrics_scroll_handle: ScrollHandle::new(),
            metrics_vertical_scroll_handle: ScrollHandle::new(),
        }
    }

    fn select_result_directory(&mut self, cx: &mut Context<Self>) {
        if self.picking_directory {
            return;
        }
        self.picking_directory = true;
        self.load_error = None;
        cx.notify();

        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Results".into()),
        });

        cx.spawn(async move |this, cx| match selected.await {
            Ok(Ok(Some(paths))) => {
                let Some(path) = paths.into_iter().next() else {
                    this.update(cx, |view, cx| {
                        view.picking_directory = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                };
                this.update(cx, |view, cx| {
                    view.load_result_directory(path, cx);
                })
                .ok();
            }
            Ok(Ok(None)) => {
                this.update(cx, |view, cx| {
                    view.picking_directory = false;
                    cx.notify();
                })
                .ok();
            }
            Ok(Err(error)) => {
                this.update(cx, |view, cx| {
                    view.picking_directory = false;
                    view.load_error = Some(format!("Could not open directory picker: {error:#}"));
                    cx.notify();
                })
                .ok();
            }
            Err(error) => {
                this.update(cx, |view, cx| {
                    view.picking_directory = false;
                    view.load_error = Some(format!("Directory picker was interrupted: {error}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn load_result_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.picking_directory = true;
        self.load_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { ResultsModel::load(path) })
                .await;
            this.update(cx, |view, cx| {
                view.picking_directory = false;
                match loaded {
                    Ok(model) => {
                        let result_directory = model.result_directory.clone();
                        view.model = Some(model);
                        view.selected_tab = 0;
                        view.load_error = None;
                        view.flamegraph_instructions = false;
                        view.flamegraph_zoom = ROOT_ID;
                        view.metrics_scroll_handle
                            .set_offset(point(px(0.0), px(0.0)));
                        view.metrics_vertical_scroll_handle
                            .set_offset(point(px(0.0), px(0.0)));
                        let _ = recent::remember(&mut view.recent_results, &result_directory);
                    }
                    Err(error) => view.load_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let chrome = div()
            .h(px(36.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(CHROME));

        let Some(model) = self.model.as_ref() else {
            return chrome;
        };

        chrome
            .child(
                div()
                    .h_full()
                    .flex()
                    .children(model.tabs.iter().enumerate().map(|(index, tab)| {
                        let selected = index == self.selected_tab;
                        div()
                            .id(SharedString::from(format!("tab-{index}")))
                            .h_full()
                            .min_w(px(104.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_3()
                            .border_r_1()
                            .border_color(rgb(BORDER))
                            .cursor_pointer()
                            .text_sm()
                            .when(selected, |element| {
                                element
                                    .bg(rgb(ACTIVE_TAB))
                                    .text_color(rgb(TEXT))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!selected, |element| {
                                element
                                    .text_color(rgb(MUTED_TEXT))
                                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            })
                            .child(tab.title().to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_tab = index;
                                this.metrics_scroll_handle
                                    .set_offset(point(px(0.0), px(0.0)));
                                this.metrics_vertical_scroll_handle
                                    .set_offset(point(px(0.0), px(0.0)));
                                cx.notify();
                            }))
                    })),
            )
            .child(div().flex_1())
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        if self.sidebar_collapsed {
            return div()
                .w(px(SIDEBAR_COLLAPSED_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(rgb(BORDER))
                .bg(rgb(CHROME))
                .child(
                    div()
                        .id("expand-projects")
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .cursor_pointer()
                        .text_color(rgb(MUTED_TEXT))
                        .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.sidebar_collapsed = false;
                            cx.notify();
                        }))
                        .child("›"),
                );
        }

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(CHROME))
            .child(
                div()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Projects")
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("collapse-projects")
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(MUTED_TEXT))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.sidebar_collapsed = true;
                                cx.notify();
                            }))
                            .child("‹"),
                    ),
            )
            .child(
                div()
                    .id("open-results-sidebar")
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(MUTED_TEXT))
                    .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.select_result_directory(cx);
                    }))
                    .child(if self.picking_directory {
                        "Opening…"
                    } else {
                        "+  Open Result…"
                    }),
            )
            .child(
                div()
                    .h(px(28.0))
                    .flex()
                    .items_end()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child("RECENT RESULTS"),
            )
            .when(self.recent_results.is_empty(), |element| {
                element.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child("No recent results"),
                )
            })
            .children(self.recent_results.iter().enumerate().map(|(index, path)| {
                let active = self
                    .model
                    .as_ref()
                    .is_some_and(|model| same_directory(&model.result_directory, path));
                let result_name = result_name(path);
                let parent = path
                    .parent()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                let path = path.clone();

                div()
                    .id(SharedString::from(format!("recent-result-{index}")))
                    .min_h(px(48.0))
                    .flex()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .cursor_pointer()
                    .text_sm()
                    .when(active, |element| element.bg(rgb(ACTIVE_TAB)))
                    .when(!active, |element| {
                        element.hover(|element| element.bg(rgb(HOVER)))
                    })
                    .child(
                        div()
                            .w(px(2.0))
                            .when(active, |element| element.bg(rgb(ACCENT))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_1()
                            .flex_col()
                            .justify_center()
                            .px_3()
                            .child(div().overflow_hidden().child(result_name))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_xs()
                                    .text_color(rgb(MUTED_TEXT))
                                    .child(parent),
                            ),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.load_result_directory(path.clone(), cx);
                    }))
            }))
            .child(div().flex_1())
            .when_some(self.load_error.clone(), |element, error| {
                element.child(
                    div()
                        .p_3()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .text_xs()
                        .text_color(rgb(0xf14c4c))
                        .child(error),
                )
            })
    }

    fn render_sidebar_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("projects-resizer")
            .w(px(5.0))
            .h_full()
            .bg(rgb(BORDER))
            .cursor_col_resize()
            .hover(|element| element.bg(rgb(ACCENT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.sidebar_resizing = true;
                    cx.notify();
                }),
            )
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> Div {
        let model = self.model.as_ref().expect("loaded results");
        let command = model
            .record_info
            .command
            .as_ref()
            .map(|command| command.join(" "))
            .filter(|command| !command.is_empty())
            .unwrap_or_else(|| "Attached process".to_string());

        let summary = div().flex().size_full().child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .child(pane_header("Performance counters"))
                .child(
                    div()
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .px_3()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child(div().flex_1().child("Counter"))
                        .child(div().w(px(150.0)).text_right().child("Value"))
                        .child(div().w(px(120.0)).text_right().child("Ratio")),
                )
                .children(model.summary.rows().into_iter().map(render_counter_row)),
        );

        summary
            .when(!self.inspector_collapsed, |element| {
                element.child(self.render_inspector_resizer(cx)).child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(self.inspector_width))
                        .bg(rgb(WORKSPACE))
                        .child(pane_header_with_control(
                            "Recording",
                            "collapse-recording",
                            "›",
                            cx.listener(|view, _, _, cx| {
                                view.inspector_collapsed = true;
                                cx.notify();
                            }),
                        ))
                        .children([
                            render_info_row("Scenario", model.record_info.scenario.name()),
                            render_info_row("Command", command),
                            render_info_row("CPU family", model.record_info.cpu_model.clone()),
                            render_info_row("CPU vendor", model.record_info.cpu_vendor.clone()),
                        ]),
                )
            })
            .when(self.inspector_collapsed, |element| {
                element.child(
                    div()
                        .w(px(INSPECTOR_COLLAPSED_WIDTH))
                        .h_full()
                        .border_l_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(CHROME))
                        .child(
                            div()
                                .id("expand-recording")
                                .h(px(32.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(rgb(MUTED_TEXT))
                                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.inspector_collapsed = false;
                                    cx.notify();
                                }))
                                .child("‹"),
                        ),
                )
            })
    }

    fn render_inspector_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("recording-resizer")
            .w(px(5.0))
            .h_full()
            .bg(rgb(BORDER))
            .cursor_col_resize()
            .hover(|element| element.bg(rgb(ACCENT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.inspector_resizing = true;
                    cx.notify();
                }),
            )
    }

    fn render_metrics_table(
        &self,
        title: &str,
        table: &MetricsTableData,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some((sticky_column, scrolling_columns)) = table.columns.split_first() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED_TEXT))
                .child("This table has no columns.");
        };
        let scrolling_width = table.total_width() - sticky_column.width;
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .h(px(32.0))
                    .min_h(px(32.0))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string())
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("metrics-scroll-left")
                            .w(px(24.0))
                            .h(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(MUTED_TEXT))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.scroll_metrics_horizontal(360.0);
                                cx.notify();
                            }))
                            .child("←"),
                    )
                    .child(
                        div()
                            .id("metrics-scroll-right")
                            .w(px(24.0))
                            .h(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(MUTED_TEXT))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.scroll_metrics_horizontal(-360.0);
                                cx.notify();
                            }))
                            .child("→"),
                    ),
            )
            .when_some(table.error.clone(), |element, error| {
                element.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_color(rgb(0xf14c4c))
                        .child(error),
                )
            })
            .when(table.error.is_none(), |element| {
                element.child(
                    div()
                        .min_h(px(0.0))
                        .flex_1()
                        .flex()
                        .child(
                            div()
                                .w(px(sticky_column.width))
                                .min_w(px(sticky_column.width))
                                .h_full()
                                .flex()
                                .flex_col()
                                .border_r_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(WORKSPACE))
                                .child(render_metrics_header(std::slice::from_ref(sticky_column)))
                                .child(
                                    div()
                                        .id("metrics-table-sticky-vertical-scroll")
                                        .min_h(px(0.0))
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.metrics_vertical_scroll_handle)
                                        .child(render_metrics_rows(
                                            table,
                                            std::slice::from_ref(sticky_column),
                                            0,
                                            "metrics-sticky-row",
                                            false,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .id("metrics-table-horizontal-scroll")
                                .min_w(px(0.0))
                                .min_h(px(0.0))
                                .flex_1()
                                .overflow_x_scroll()
                                .track_scroll(&self.metrics_scroll_handle)
                                .child(
                                    div()
                                        .w(px(scrolling_width))
                                        .h_full()
                                        .flex()
                                        .flex_col()
                                        .child(render_metrics_header(scrolling_columns))
                                        .child(
                                            div()
                                                .id("metrics-table-vertical-scroll")
                                                .min_h(px(0.0))
                                                .flex_1()
                                                .overflow_y_scroll()
                                                .track_scroll(&self.metrics_vertical_scroll_handle)
                                                .child(render_metrics_rows(
                                                    table,
                                                    scrolling_columns,
                                                    1,
                                                    "metrics-row",
                                                    true,
                                                )),
                                        ),
                                ),
                        ),
                )
            })
    }

    fn scroll_metrics_horizontal(&self, delta: f32) {
        let offset = self.metrics_scroll_handle.offset();
        let max_offset = self.metrics_scroll_handle.max_offset();
        let next_x = (f32::from(offset.x) + delta).clamp(-f32::from(max_offset.width), 0.0);
        self.metrics_scroll_handle
            .set_offset(point(px(next_x), offset.y));
    }

    fn render_flamegraph(&self, flamegraph: &FlamegraphData, cx: &mut Context<Self>) -> Div {
        let has_cycles = flamegraph.cycles.is_some();
        let has_instructions = flamegraph.instructions.is_some();
        let layout = flamegraph.layout(self.flamegraph_instructions, self.flamegraph_zoom);

        let toolbar = div()
            .h(px(34.0))
            .flex()
            .items_center()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(CHROME))
            .child(metric_toggle(
                "Cycles",
                !self.flamegraph_instructions,
                has_cycles,
                cx.listener(|view, _, _, cx| {
                    view.flamegraph_instructions = false;
                    view.flamegraph_zoom = ROOT_ID;
                    cx.notify();
                }),
            ))
            .child(metric_toggle(
                "Instructions",
                self.flamegraph_instructions,
                has_instructions,
                cx.listener(|view, _, _, cx| {
                    view.flamegraph_instructions = true;
                    view.flamegraph_zoom = ROOT_ID;
                    cx.notify();
                }),
            ))
            .child(div().flex_1())
            .when(self.flamegraph_zoom != ROOT_ID, |element| {
                element.child(
                    div()
                        .id("reset-flamegraph-zoom")
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .px_2()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.flamegraph_zoom = ROOT_ID;
                            cx.notify();
                        }))
                        .child("Reset zoom"),
                )
            });

        let Some(layout) = layout else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(pane_header("Flamegraph"))
                .child(toolbar)
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_color(rgb(0xf14c4c))
                        .child(
                            flamegraph
                                .error
                                .clone()
                                .unwrap_or_else(|| "No flamegraph data available".to_string()),
                        ),
                );
        };

        let graph_height = ((layout.max_depth + 1) as f32 * FLAME_FRAME_HEIGHT).max(260.0);
        let status = format!(
            "{} · {} {}",
            layout.root_name,
            layout.total.to_formatted_string(&num_format::Locale::en),
            if self.flamegraph_instructions {
                "instructions"
            } else {
                "cycles"
            }
        );
        let max_depth = layout.max_depth;
        let flame_total = layout.total;

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(pane_header("Flamegraph"))
            .child(toolbar)
            .child(
                div()
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(status),
            )
            .child(
                div()
                    .id("flamegraph-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .p_2()
                    .child(
                        div()
                            .relative()
                            .min_w(px(900.0))
                            .w_full()
                            .h(px(graph_height))
                            .children(layout.frames.into_iter().map(|frame| {
                                let top = (max_depth.saturating_sub(frame.depth)) as f32
                                    * FLAME_FRAME_HEIGHT;
                                let color = flame_frame_color(&frame.name, frame.depth);
                                let label = if frame.width > 0.025 {
                                    format!(
                                        "{}  {:.1}%",
                                        frame.name,
                                        frame.value as f64 / flame_total as f64 * 100.0
                                    )
                                } else {
                                    String::new()
                                };
                                let id = frame.id;

                                div()
                                    .id(SharedString::from(format!("flame-frame-{id}")))
                                    .absolute()
                                    .left(relative(frame.x))
                                    .top(px(top))
                                    .w(relative(frame.width.max(0.000_5)))
                                    .h(px(FLAME_FRAME_HEIGHT - 1.0))
                                    .flex()
                                    .items_center()
                                    .px_1()
                                    .border_1()
                                    .border_color(rgb(WORKSPACE))
                                    .bg(rgb(color))
                                    .cursor_pointer()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(rgb(0xf1f1f1))
                                    .hover(|element| element.border_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.flamegraph_zoom = id;
                                        cx.notify();
                                    }))
                                    .child(label)
                            })),
                    ),
            )
    }

    fn render_loops_placeholder(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(pane_header("Loops"))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED_TEXT))
                    .child("No loop data is available for this recording."),
            )
    }

    fn render_result_picker(&self, cx: &mut Context<Self>) -> Div {
        let button_label = if self.picking_directory {
            "Selecting…"
        } else {
            "Open results…"
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_sm()
            .child(
                div()
                    .mb_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open an mperf recording"),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(MUTED_TEXT))
                    .child("Choose a recent result or open a directory."),
            )
            .child(
                div()
                    .id("open-results")
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(ACTIVE_TAB))
                    .when(!self.picking_directory, |element| {
                        element
                            .cursor_pointer()
                            .hover(|element| element.bg(rgb(HOVER)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.select_result_directory(cx);
                            }))
                    })
                    .child(button_label),
            )
    }
}

impl Render for MperfGui {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.model.as_ref() {
            Some(model) => match model.tabs.get(self.selected_tab) {
                Some(GuiTab::Summary) => self.render_summary(cx).into_any_element(),
                Some(GuiTab::MetricsTable { title, data }) => self
                    .render_metrics_table(title, data, cx)
                    .into_any_element(),
                Some(GuiTab::Flamegraph(flamegraph)) => {
                    self.render_flamegraph(flamegraph, cx).into_any_element()
                }
                Some(GuiTab::Loops) => self.render_loops_placeholder().into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED_TEXT))
                    .child("This recording did not define any result tabs.")
                    .into_any_element(),
            },
            None => self.render_result_picker(cx).into_any_element(),
        };

        let root = div()
            .id("application-root")
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(WORKSPACE))
            .text_color(rgb(TEXT))
            .child(self.render_sidebar(cx))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                if view.sidebar_resizing && event.dragging() {
                    view.sidebar_width =
                        f32::from(event.position.x).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                    cx.notify();
                }
                if view.inspector_resizing && event.dragging() {
                    view.inspector_width =
                        f32::from(window.viewport_size().width - event.position.x)
                            .clamp(INSPECTOR_MIN_WIDTH, INSPECTOR_MAX_WIDTH);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    if view.sidebar_resizing {
                        view.sidebar_resizing = false;
                        cx.notify();
                    }
                    if view.inspector_resizing {
                        view.inspector_resizing = false;
                        cx.notify();
                    }
                }),
            );

        root.when(!self.sidebar_collapsed, |element| {
            element.child(self.render_sidebar_resizer(cx))
        })
        .child(
            div()
                .min_w_0()
                .h_full()
                .flex()
                .flex_1()
                .flex_col()
                .child(self.render_tabs(cx))
                .child(
                    div()
                        .id("result-content")
                        .min_h(px(0.0))
                        .flex_1()
                        .overflow_hidden()
                        .child(content),
                ),
        )
    }
}

fn same_directory(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
}

fn result_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn pane_header(title: impl Into<SharedString>) -> Div {
    div()
        .h(px(32.0))
        .flex()
        .items_center()
        .px_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

fn pane_header_with_control(
    title: impl Into<SharedString>,
    control_id: &'static str,
    control_label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(32.0))
        .flex()
        .items_center()
        .px_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
        .child(div().flex_1())
        .child(
            div()
                .id(control_id)
                .w(px(22.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .font_weight(FontWeight::NORMAL)
                .text_color(rgb(MUTED_TEXT))
                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                .on_click(on_click)
                .child(control_label),
        )
}

fn render_metrics_header(columns: &[MetricsColumn]) -> Div {
    div()
        .h(px(30.0))
        .min_h(px(30.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_sm()
        .text_color(rgb(MUTED_TEXT))
        .children(
            columns
                .iter()
                .map(|column| render_metrics_cell(column, column.label.clone())),
        )
}

fn render_metrics_rows(
    table: &MetricsTableData,
    columns: &[MetricsColumn],
    value_offset: usize,
    id_prefix: &'static str,
    show_empty_message: bool,
) -> Div {
    div()
        .w(px(columns.iter().map(|column| column.width).sum::<f32>()))
        .min_h(px(table.rows.len().max(1) as f32 * 30.0))
        .flex()
        .flex_col()
        .when(table.rows.is_empty() && show_empty_message, |element| {
            element.child(
                div()
                    .h(px(120.0))
                    .min_h(px(120.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED_TEXT))
                    .child("No hotspot data"),
            )
        })
        .children(table.rows.iter().enumerate().map(|(index, row)| {
            div()
                .id(SharedString::from(format!("{id_prefix}-{index}")))
                .h(px(30.0))
                .min_h(px(30.0))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_sm()
                .hover(|element| element.bg(rgb(HOVER)))
                .children(
                    columns
                        .iter()
                        .zip(row.values.iter().skip(value_offset))
                        .map(|(column, value)| render_metrics_cell(column, value.clone())),
                )
        }))
}

fn render_metrics_cell(column: &MetricsColumn, value: String) -> Div {
    div()
        .w(px(column.width))
        .min_w(px(column.width))
        .h_full()
        .flex()
        .items_center()
        .px_3()
        .border_r_1()
        .border_color(rgb(BORDER))
        .truncate()
        .when(column.align_right, |element| {
            element.justify_end().text_right()
        })
        .child(value)
}

fn metric_toggle(
    label: &'static str,
    selected: bool,
    available: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "flamegraph-{}",
            label.to_ascii_lowercase()
        )))
        .h(px(24.0))
        .flex()
        .items_center()
        .px_2()
        .border_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .when(selected, |element| {
            element
                .bg(rgb(ACTIVE_TAB))
                .text_color(rgb(TEXT))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |element| element.text_color(rgb(MUTED_TEXT)))
        .when(available, |element| {
            element
                .cursor_pointer()
                .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                .on_click(on_click)
        })
        .when(!available, |element| element.opacity(0.45))
        .child(label)
}

fn flame_frame_color(name: &str, depth: usize) -> u32 {
    let mut hash = depth as u64;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(16_777_619) ^ u64::from(byte);
    }
    const PALETTE: [u32; 8] = [
        0x8b5e34, 0x9a6b3d, 0x7b5f43, 0x76604a, 0x795548, 0x6d5d4b, 0x73573d, 0x806044,
    ];
    PALETTE[hash as usize % PALETTE.len()]
}

fn render_counter_row(row: CounterRow) -> Div {
    div()
        .h(px(31.0))
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
                .w(px(120.0))
                .text_right()
                .text_color(rgb(MUTED_TEXT))
                .child(row.detail),
        )
}

fn render_info_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .min_h(px(34.0))
        .flex()
        .items_start()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_sm()
        .hover(|element| element.bg(rgb(HOVER)))
        .child(
            div()
                .w(px(88.0))
                .text_color(rgb(MUTED_TEXT))
                .child(label.into()),
        )
        .child(div().flex_1().child(value.into()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let model = cli.result_directory.map(ResultsModel::load).transpose()?;
    let mut recent_results = recent::load();
    if let Some(model) = model.as_ref() {
        let _ = recent::remember(&mut recent_results, &model.result_directory);
    }
    let should_select = model.is_none();

    Application::new().run(move |cx: &mut App| {
        cx.init_colors();
        let bounds = Bounds::centered(None, size(px(1180.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.0), px(520.0))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = MperfGui::new(model, recent_results);
                    if should_select {
                        view.select_result_directory(cx);
                    }
                    view
                })
            },
        )
        .expect("failed to open mperf GUI window");
        cx.activate(true);
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn result_directory_is_optional() {
        let cli = Cli::try_parse_from(["mperf-gui"]).unwrap();
        assert!(cli.result_directory.is_none());
    }
}
