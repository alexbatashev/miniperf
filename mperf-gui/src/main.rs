mod analysis_cache;
mod flamegraph;
mod memory;
mod metrics;
mod model;
mod profile;
mod profile_analysis;
mod recent;
mod roofline;
mod snapshot;
mod source;
mod sql;
mod theme;
mod views;

use std::{
    borrow::Cow,
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use flamelens::flame::{ROOT_ID, StackIdentifier};
use gpui::{
    App, Application, AssetSource, Bounds, Context, MouseButton, MouseMoveEvent, PathPromptOptions,
    ScrollHandle, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, point,
    prelude::*, px, size,
};

use flamegraph::FlamegraphData;
use model::{GuiTab, ResultsModel};
use source::{SourceDocument, SourceLocation};
use theme::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum VisualizationKind {
    Resources,
    Memory,
    Roofline,
    #[default]
    Flamegraph,
    FlameScope,
    Icicle,
    StackTimeline,
    CpuHeatmap,
    CallersAndCallees,
    MetricsTimeline,
}

impl VisualizationKind {
    const ALL: [Self; 10] = [
        Self::Resources,
        Self::Memory,
        Self::Roofline,
        Self::Flamegraph,
        Self::FlameScope,
        Self::Icicle,
        Self::StackTimeline,
        Self::CpuHeatmap,
        Self::CallersAndCallees,
        Self::MetricsTimeline,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Resources => "Resources (USE)",
            Self::Memory => "Memory",
            Self::Roofline => "Roofline",
            Self::Flamegraph => "Flamegraph",
            Self::FlameScope => "FlameScope",
            Self::Icicle => "Icicle",
            Self::StackTimeline => "Stack Timeline",
            Self::CpuHeatmap => "CPU Heatmap",
            Self::CallersAndCallees => "Callers & Callees",
            Self::MetricsTimeline => "Metrics Timeline",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Resources => "resources-use",
            Self::Memory => "memory",
            Self::Roofline => "roofline",
            Self::Flamegraph => "flamegraph",
            Self::FlameScope => "flamescope",
            Self::Icicle => "icicle",
            Self::StackTimeline => "stack-timeline",
            Self::CpuHeatmap => "cpu-heatmap",
            Self::CallersAndCallees => "callers-callees",
            Self::MetricsTimeline => "metrics-timeline",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LeftPaneKind {
    #[default]
    Recordings,
    AvailableViews,
}

#[derive(Parser)]
#[command(about = "GPU-accelerated viewer for mperf result directories")]
struct Cli {
    /// Directory containing info.json and perf.db.
    result_directory: Option<PathBuf>,
}

#[derive(Clone)]
struct HoveredFrame {
    id: StackIdentifier,
    name: String,
    value: u64,
    self_value: u64,
}

struct HotspotFilter {
    root: String,
    frame_ids: BTreeSet<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum BottomPanelKind {
    Statistics,
    #[default]
    Hotspots,
}

impl BottomPanelKind {
    fn title(self) -> &'static str {
        match self {
            Self::Statistics => "Statistics",
            Self::Hotspots => "Hotspots",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Statistics => "▥",
            Self::Hotspots => "♨",
        }
    }
}

struct MperfGui {
    model: Option<ResultsModel>,
    caches: analysis_cache::AnalysisCaches,
    recent_results: Vec<PathBuf>,
    picking_directory: bool,
    load_error: Option<String>,

    sidebar_width: f32,
    sidebar_collapsed: bool,
    sidebar_resizing: bool,
    active_left_pane: LeftPaneKind,

    bottom_panel_height: f32,
    bottom_panel_collapsed: bool,
    bottom_panel_resizing: bool,
    active_bottom_panel: BottomPanelKind,
    /// Overrides for hotspot table column widths; empty means defaults.
    hotspot_column_widths: Vec<f32>,
    /// Active header drag as (column, start x, starting width).
    hotspot_column_drag: Option<(usize, f32, f32)>,

    flamegraph_instructions: bool,
    flamegraph_zoom: StackIdentifier,
    flamegraph_scroll_handle: ScrollHandle,
    filtered_flamegraph_scroll_handle: ScrollHandle,
    selected_stack: Option<StackIdentifier>,
    selected_function: Option<String>,
    hotspot_filter: Option<HotspotFilter>,
    hovered_frame: Option<HoveredFrame>,

    open_visualizations: Vec<VisualizationKind>,
    active_visualization: Option<VisualizationKind>,
    snapshot_resource: Option<String>,
    selected_time_range: Option<profile::TimeRange>,
    icicle_focus_path: Vec<usize>,
    cpu_heatmap_bucket_ns: Option<u64>,
    selected_roofline_loop: Option<usize>,
    hovered_roofline_roof: Option<usize>,
    roofline_labels_always_visible: bool,
    roofline_chart_size: Option<(f32, f32)>,
    roofline_loop_scroll_handle: ScrollHandle,

    source_documents: Vec<SourceDocument>,
    active_source: Option<usize>,

    metrics_scroll_handle: ScrollHandle,
    metrics_vertical_scroll_handle: ScrollHandle,
}

impl MperfGui {
    fn new(model: Option<ResultsModel>, recent_results: Vec<PathBuf>) -> Self {
        let mut view = Self {
            model,
            caches: analysis_cache::AnalysisCaches::default(),
            recent_results,
            picking_directory: false,
            load_error: None,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            sidebar_collapsed: true,
            sidebar_resizing: false,
            active_left_pane: LeftPaneKind::default(),
            bottom_panel_height: BOTTOM_PANEL_DEFAULT_HEIGHT,
            bottom_panel_collapsed: true,
            bottom_panel_resizing: false,
            active_bottom_panel: BottomPanelKind::default(),
            hotspot_column_widths: Vec::new(),
            hotspot_column_drag: None,
            flamegraph_instructions: false,
            flamegraph_zoom: ROOT_ID,
            flamegraph_scroll_handle: ScrollHandle::new(),
            filtered_flamegraph_scroll_handle: ScrollHandle::new(),
            selected_stack: None,
            selected_function: None,
            hotspot_filter: None,
            hovered_frame: None,
            open_visualizations: Vec::new(),
            active_visualization: None,
            snapshot_resource: None,
            selected_time_range: None,
            icicle_focus_path: Vec::new(),
            cpu_heatmap_bucket_ns: None,
            selected_roofline_loop: None,
            hovered_roofline_roof: None,
            roofline_labels_always_visible: false,
            roofline_chart_size: None,
            roofline_loop_scroll_handle: ScrollHandle::new(),
            source_documents: Vec::new(),
            active_source: None,
            metrics_scroll_handle: ScrollHandle::new(),
            metrics_vertical_scroll_handle: ScrollHandle::new(),
        };
        view.reset_workspace();
        view
    }

    fn flamegraph_data(&self) -> Option<&FlamegraphData> {
        self.model.as_ref()?.tabs.iter().find_map(|tab| match tab {
            GuiTab::Flamegraph(data) => Some(data.as_ref()),
            _ => None,
        })
    }

    fn roofline_data(&self) -> Option<&roofline::RooflineData> {
        self.model.as_ref()?.tabs.iter().find_map(|tab| match tab {
            GuiTab::Loops(data) => Some(data.as_ref()),
            _ => None,
        })
    }

    fn memory_data(&self) -> Option<&memory::MemoryData> {
        self.model.as_ref()?.tabs.iter().find_map(|tab| match tab {
            GuiTab::Memory(data) => Some(data.as_ref()),
            _ => None,
        })
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
            prompt: Some("Open Recording".into()),
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
                this.update(cx, |view, cx| view.load_result_directory(path, cx))
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
                        view.reset_workspace();
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

    fn reset_workspace(&mut self) {
        self.caches.clear();
        self.flamegraph_instructions = false;
        self.flamegraph_zoom = ROOT_ID;
        self.selected_stack = None;
        self.selected_function = None;
        self.hotspot_filter = None;
        self.hovered_frame = None;
        self.active_bottom_panel = BottomPanelKind::default();
        self.bottom_panel_collapsed = true;
        self.open_visualizations.clear();
        self.active_visualization = None;
        self.snapshot_resource = None;
        let snapshot_scenario = self
            .model
            .as_ref()
            .is_some_and(|model| model.record_info.scenario == mperf_data::Scenario::Snapshot);
        let memory_scenario = self
            .model
            .as_ref()
            .is_some_and(|model| model.record_info.scenario == mperf_data::Scenario::Mem);
        if snapshot_scenario && self.visualization_available(VisualizationKind::Resources) {
            self.open_visualizations.push(VisualizationKind::Resources);
            self.active_visualization = Some(VisualizationKind::Resources);
        } else if memory_scenario && self.visualization_available(VisualizationKind::Memory) {
            self.open_visualizations.push(VisualizationKind::Memory);
            self.active_visualization = Some(VisualizationKind::Memory);
        } else if self.visualization_available(VisualizationKind::Roofline) {
            self.open_visualizations.push(VisualizationKind::Roofline);
            self.active_visualization = Some(VisualizationKind::Roofline);
        } else if self.visualization_available(VisualizationKind::Memory) {
            self.open_visualizations.push(VisualizationKind::Memory);
            self.active_visualization = Some(VisualizationKind::Memory);
        } else if self.visualization_available(VisualizationKind::Flamegraph) {
            self.open_visualizations.push(VisualizationKind::Flamegraph);
            self.active_visualization = Some(VisualizationKind::Flamegraph);
        }
        self.selected_time_range = None;
        self.icicle_focus_path.clear();
        self.cpu_heatmap_bucket_ns = None;
        self.selected_roofline_loop = None;
        self.hovered_roofline_roof = None;
        self.roofline_chart_size = None;
        self.roofline_loop_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.source_documents.clear();
        self.active_source = None;
        self.metrics_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.metrics_vertical_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn toggle_bottom_panel(&mut self, panel: BottomPanelKind) {
        if !self.bottom_panel_collapsed && self.active_bottom_panel == panel {
            self.bottom_panel_collapsed = true;
            return;
        }
        self.active_bottom_panel = panel;
        self.bottom_panel_collapsed = false;
        self.metrics_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.metrics_vertical_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn select_visualization(&mut self, visualization: VisualizationKind) {
        if !self.visualization_available(visualization) {
            return;
        }
        self.activate_visualization(visualization);
    }

    fn activate_visualization(&mut self, visualization: VisualizationKind) {
        if !self.open_visualizations.contains(&visualization) {
            self.open_visualizations.push(visualization);
        }
        self.active_source = None;
        self.active_visualization = Some(visualization);
    }

    fn close_visualization(&mut self, visualization: VisualizationKind) {
        let Some(index) = self
            .open_visualizations
            .iter()
            .position(|open| *open == visualization)
        else {
            return;
        };
        self.open_visualizations.remove(index);
        if self.active_visualization != Some(visualization) {
            return;
        }
        self.active_visualization = self
            .open_visualizations
            .get(index)
            .or_else(|| self.open_visualizations.last())
            .copied();
        if self.active_visualization.is_none()
            && self.active_source.is_none()
            && !self.source_documents.is_empty()
        {
            self.active_source = Some(0);
        }
    }

    fn toggle_left_pane(&mut self, pane: LeftPaneKind) {
        if !self.sidebar_collapsed && self.active_left_pane == pane {
            self.sidebar_collapsed = true;
            self.sidebar_resizing = false;
            return;
        }
        self.active_left_pane = pane;
        self.sidebar_collapsed = false;
    }

    fn available_visualizations(&self) -> Vec<VisualizationKind> {
        VisualizationKind::ALL
            .into_iter()
            .filter(|visualization| self.visualization_available(*visualization))
            .collect()
    }

    fn visualization_available(&self, visualization: VisualizationKind) -> bool {
        let Some(model) = self.model.as_ref() else {
            return false;
        };
        match visualization {
            VisualizationKind::Resources => return model.snapshot.is_some(),
            VisualizationKind::Memory => return self.memory_data().is_some(),
            VisualizationKind::Roofline => return self.roofline_data().is_some(),
            VisualizationKind::Flamegraph => {
                return self.flamegraph_data().is_some_and(|flamegraph| {
                    flamegraph.cycles.is_some() || flamegraph.instructions.is_some()
                });
            }
            _ => {}
        }
        if model.profile.error.is_some() {
            return false;
        }

        // This runs while rendering the Available Views pane, so keep it a
        // capability check rather than rebuilding every analysis on each
        // frame. The selected view performs its full analysis when opened.
        let has_samples = !model.profile.samples.is_empty();
        let has_stack_samples = model
            .profile
            .samples
            .iter()
            .any(|sample| !sample.stack.is_empty());
        match visualization {
            VisualizationKind::Resources
            | VisualizationKind::Memory
            | VisualizationKind::Roofline
            | VisualizationKind::Flamegraph => unreachable!(),
            VisualizationKind::FlameScope => has_samples,
            VisualizationKind::Icicle
            | VisualizationKind::StackTimeline
            | VisualizationKind::CallersAndCallees => has_stack_samples,
            VisualizationKind::CpuHeatmap => !model.profile.cpu_observations.is_empty(),
            VisualizationKind::MetricsTimeline => {
                !model.profile.counter_metrics.is_empty()
                    && model
                        .profile
                        .samples
                        .iter()
                        .any(|sample| sample.counters.iter().any(Option::is_some))
            }
        }
    }

    fn select_time_range(&mut self, range: profile::TimeRange) {
        self.selected_time_range = Some(range);
        self.icicle_focus_path.clear();
    }

    fn profile_sample_filter(&self) -> profile_analysis::SampleFilter {
        profile_analysis::SampleFilter {
            range: self.selected_time_range,
            frame_ids: self
                .hotspot_filter
                .as_ref()
                .map(|filter| filter.frame_ids.clone()),
            ..Default::default()
        }
    }

    fn select_profile_function(&mut self, frame_id: usize) {
        let Some(function) = self
            .model
            .as_ref()
            .and_then(|model| model.profile.frames.get(frame_id))
            .map(|frame| frame.name.clone())
        else {
            return;
        };
        let selected_stack = self.flamegraph_data().and_then(|flamegraph| {
            flamegraph.hottest_stack_named(self.flamegraph_instructions, &function)
        });
        self.selected_stack = selected_stack;
        self.flamegraph_zoom = selected_stack.unwrap_or(ROOT_ID);
        self.selected_function = Some(function.clone());
        self.hotspot_filter = Some(HotspotFilter {
            root: function.clone(),
            frame_ids: BTreeSet::from([frame_id]),
        });
        self.active_source = None;
        self.icicle_focus_path.clear();
    }

    fn clear_global_filter(&mut self) {
        self.selected_time_range = None;
        self.selected_stack = None;
        self.selected_function = None;
        self.hotspot_filter = None;
        self.flamegraph_zoom = ROOT_ID;
        self.icicle_focus_path.clear();
    }

    fn select_roofline_loop(&mut self, index: usize, open_source: bool) {
        let source = self
            .roofline_data()
            .and_then(|data| data.loops.get(index))
            .and_then(roofline::RooflineLoop::source);
        if self
            .roofline_data()
            .is_none_or(|data| index >= data.loops.len())
        {
            return;
        }
        self.selected_roofline_loop = Some(index);
        self.roofline_loop_scroll_handle.scroll_to_item(index);
        self.active_source = None;
        if open_source && let Some(source) = source {
            self.open_source(source);
        }
    }

    fn select_flame_frame(&mut self, id: StackIdentifier, function: String) {
        // Flamelens inserts a synthetic root above every folded stack. It has
        // no matching profile frame, so treating it as a function selection
        // would install an intentionally empty frame filter and hide every
        // coordinated view.
        if id == ROOT_ID {
            return;
        }

        let frame_ids = self
            .model
            .as_ref()
            .map(|model| {
                model
                    .profile
                    .frames
                    .iter()
                    .filter(|frame| profile_names_match(&frame.name, &function))
                    .map(|frame| frame.id)
                    .collect()
            })
            .unwrap_or_default();
        self.flamegraph_zoom = id;
        self.selected_stack = Some(id);
        self.selected_function = Some(function.clone());
        self.hotspot_filter = Some(HotspotFilter {
            root: function,
            frame_ids,
        });
        self.active_source = None;
        self.icicle_focus_path.clear();
    }

    fn open_source(&mut self, location: SourceLocation) {
        if let Some(index) = self
            .source_documents
            .iter()
            .position(|document| document.matches(&location.path))
        {
            self.source_documents[index].focus_line = location.line;
            self.source_documents[index]
                .scroll_handle
                .scroll_to_item(location.line.saturating_sub(4));
            self.active_source = Some(index);
            return;
        }

        self.source_documents.push(SourceDocument::load(location));
        self.active_source = Some(self.source_documents.len() - 1);
    }

    fn close_source(&mut self, index: usize) {
        if index >= self.source_documents.len() {
            return;
        }
        let was_active = self.active_source == Some(index);
        self.source_documents.remove(index);
        self.active_source = match self.active_source {
            Some(_) if was_active && index < self.source_documents.len() => Some(index),
            Some(_) if was_active && index > 0 => Some(index - 1),
            Some(_) if was_active => None,
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
    }
}

impl Render for MperfGui {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .id("application-body")
            .min_h(px(0.0))
            .flex()
            .flex_1()
            .overflow_hidden()
            .when(!self.sidebar_collapsed, |element| {
                element
                    .child(self.render_sidebar(cx))
                    .child(self.render_sidebar_resizer(cx))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_1()
                    .flex_col()
                    .child(self.render_document_bar(cx))
                    .child(self.render_workspace(cx))
                    .when(self.model.is_some(), |element| {
                        element
                            .child(self.render_bottom_resizer(cx))
                            .child(self.render_bottom_panel(cx))
                    }),
            );

        div()
            .id("application-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(WORKSPACE))
            .text_color(gpui::rgb(TEXT))
            .child(self.render_title_bar(cx))
            .child(body)
            .child(self.render_status_bar(cx))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                if view.sidebar_resizing && event.dragging() {
                    view.sidebar_width =
                        f32::from(event.position.x).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                    cx.notify();
                }
                if view.bottom_panel_resizing && event.dragging() {
                    view.bottom_panel_height =
                        (f32::from(window.viewport_size().height - event.position.y)
                            - STATUS_BAR_HEIGHT)
                            .clamp(BOTTOM_PANEL_MIN_HEIGHT, BOTTOM_PANEL_MAX_HEIGHT);
                    cx.notify();
                }
                if event.dragging() && view.drag_hotspot_column(f32::from(event.position.x)) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.sidebar_resizing = false;
                    view.bottom_panel_resizing = false;
                    view.hotspot_column_drag = None;
                    cx.notify();
                }),
            )
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
}

fn profile_names_match(left: &str, right: &str) -> bool {
    left == right || left.strip_prefix('_') == Some(right) || right.strip_prefix('_') == Some(left)
}

fn result_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

struct MperfAssets;

impl AssetSource for MperfAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(roofline::roofline_label_svg(path).map(Cow::Owned))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let initial_directory = cli.result_directory;
    let recent_results = recent::load();

    Application::new()
        .with_assets(MperfAssets)
        .run(move |cx: &mut App| {
            cx.init_colors();
            let bounds = Bounds::centered(None, size(px(1120.0), px(720.0)), cx);
            cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("mperf".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(10.0), px(11.0))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(760.0), px(520.0))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut view = MperfGui::new(None, recent_results);
                        match initial_directory {
                            Some(path) => view.load_result_directory(path, cx),
                            None => view.select_result_directory(cx),
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
    use std::collections::BTreeSet;

    use super::{Cli, HotspotFilter, MperfGui, ROOT_ID, VisualizationKind};
    use clap::Parser;

    #[test]
    fn result_directory_is_optional() {
        assert!(Cli::try_parse_from(["mperf-gui"]).is_ok());
    }

    #[test]
    fn activating_an_open_visualization_does_not_duplicate_its_tab() {
        let mut view = MperfGui::new(None, Vec::new());

        view.activate_visualization(VisualizationKind::Icicle);
        view.activate_visualization(VisualizationKind::Flamegraph);
        view.activate_visualization(VisualizationKind::Icicle);

        assert_eq!(
            view.open_visualizations,
            vec![VisualizationKind::Icicle, VisualizationKind::Flamegraph]
        );
        assert_eq!(view.active_visualization, Some(VisualizationKind::Icicle));
    }

    #[test]
    fn closing_the_active_visualization_activates_its_neighbor() {
        let mut view = MperfGui::new(None, Vec::new());
        view.activate_visualization(VisualizationKind::Flamegraph);
        view.activate_visualization(VisualizationKind::Icicle);
        view.activate_visualization(VisualizationKind::CpuHeatmap);
        view.activate_visualization(VisualizationKind::Icicle);

        view.close_visualization(VisualizationKind::Icicle);

        assert_eq!(
            view.open_visualizations,
            vec![VisualizationKind::Flamegraph, VisualizationKind::CpuHeatmap]
        );
        assert_eq!(
            view.active_visualization,
            Some(VisualizationKind::CpuHeatmap)
        );
    }

    #[test]
    fn an_unresolved_folded_symbol_remains_an_empty_frame_filter() {
        let mut view = MperfGui::new(None, Vec::new());
        view.hotspot_filter = Some(HotspotFilter {
            root: "missing".to_string(),
            frame_ids: BTreeSet::new(),
        });

        assert_eq!(
            view.profile_sample_filter().frame_ids,
            Some(BTreeSet::new())
        );
    }

    #[test]
    fn selecting_the_synthetic_flamegraph_root_does_not_create_a_global_filter() {
        let mut view = MperfGui::new(None, Vec::new());

        view.select_flame_frame(ROOT_ID, "all".to_string());

        assert!(view.hotspot_filter.is_none());
        assert!(view.selected_stack.is_none());
        assert!(view.selected_function.is_none());
        assert_eq!(view.profile_sample_filter().frame_ids, None);
    }

    #[test]
    fn analysis_selection_does_not_force_open_a_dock() {
        let mut view = MperfGui::new(None, Vec::new());

        view.select_flame_frame(1, "foo".to_string());

        assert!(view.sidebar_collapsed);
        assert!(view.bottom_panel_collapsed);
        assert_eq!(view.selected_function.as_deref(), Some("foo"));
    }
}
