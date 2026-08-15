use gpui::{
    Bounds, Context, Div, FontWeight, PathBuilder, Pixels, SharedString, Window, canvas, div, fill,
    point, prelude::*, px, rgb,
};

use crate::{
    MperfGui,
    snapshot::{
        ResourceUse, Severity, SnapshotChart, SnapshotData, UseCategory, format_bytes, format_count,
    },
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION, SURFACE, TEXT, TOOLBAR,
        WORKSPACE,
    },
};

const SERIES_PALETTE: [u32; 8] = [
    0xd2a45f, 0x7aa2f7, 0x9ece6a, 0xf7768e, 0xbb9af7, 0x7dcfff, 0xe0af68, 0x73daca,
];

impl MperfGui {
    pub(crate) fn render_resources_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(snapshot) = self
            .model
            .as_ref()
            .and_then(|model| model.snapshot.as_ref())
        else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED_TEXT))
                .child("This recording has no USE resource snapshot.");
        };

        let selected = self.selected_snapshot_resource(snapshot);
        let header = self.render_resources_header(snapshot);
        let findings = render_findings(snapshot);
        let grid = self.render_use_grid(snapshot, &selected, cx);
        let charts = render_resource_charts(snapshot, &selected);
        let collectors = render_collectors(snapshot);

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("resources-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_y_scroll()
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(findings)
                .child(grid)
                .child(charts)
                .child(collectors),
        )
    }

    fn selected_snapshot_resource(&self, snapshot: &SnapshotData) -> String {
        self.snapshot_resource
            .as_ref()
            .filter(|resource| {
                snapshot
                    .resources
                    .iter()
                    .any(|candidate| candidate.resource == **resource)
            })
            .cloned()
            .or_else(|| {
                // Default to the most severe resource so problems surface first.
                snapshot
                    .resources
                    .iter()
                    .max_by_key(|resource| resource.severity)
                    .map(|resource| resource.resource.clone())
            })
            .unwrap_or_default()
    }

    fn render_resources_header(&self, snapshot: &SnapshotData) -> Div {
        let available = snapshot
            .collectors
            .iter()
            .filter(|collector| collector.status == "available")
            .count();
        let status = format!(
            "{} recorded · ~{} samples per metric · {}/{} collectors available",
            format_seconds(snapshot.duration_ns as f64 / 1e9),
            snapshot.sample_count,
            available,
            snapshot.collectors.len().max(available),
        );
        div()
            .h(px(28.0))
            .min_h(px(28.0))
            .flex()
            .items_center()
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(TOOLBAR))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Resources · USE method"),
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
    }

    fn render_use_grid(
        &self,
        snapshot: &SnapshotData,
        selected: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(snapshot.resources.iter().map(|resource| {
                let is_selected = resource.resource == selected;
                let name = resource.resource.clone();
                div()
                    .id(SharedString::from(format!("use-resource-{}", name)))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if is_selected { SELECTION } else { BORDER }))
                    .bg(rgb(SURFACE))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(ACCENT)))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.snapshot_resource = Some(name.clone());
                        cx.notify();
                    }))
                    .child(resource_card(resource, is_selected))
            }))
    }
}

fn resource_card(resource: &ResourceUse, selected: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(30.0))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .border_b_1()
                .border_color(rgb(BORDER))
                .bg(rgb(CHROME))
                .child(severity_dot(resource.severity))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(selected, |element| element.text_color(rgb(SELECTION)))
                        .child(display_resource(&resource.resource).to_string()),
                )
                .when_some(resource.headline.clone(), |element, headline| {
                    element.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(TEXT))
                            .child(headline),
                    )
                }),
        )
        .child(
            div().flex().children(
                UseCategory::ALL
                    .iter()
                    .enumerate()
                    .map(|(index, category)| {
                        let metrics = &resource.summaries[index];
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .p_2()
                            .when(index > 0, |element| {
                                element.border_l_1().border_color(rgb(BORDER))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(MUTED_TEXT))
                                    .child(category.title()),
                            )
                            .when(metrics.is_empty(), |element| {
                                element.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED_TEXT))
                                        .child("no signals recorded"),
                                )
                            })
                            .children(metrics.iter().map(|metric| {
                                div()
                                    .flex()
                                    .gap_1()
                                    .text_xs()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .text_color(rgb(MUTED_TEXT))
                                            .child(metric.metric.replace('_', " ")),
                                    )
                                    .child(div().text_color(rgb(TEXT)).child(metric.value.clone()))
                                    .child(scope_tag(&metric.scope))
                            }))
                    }),
            ),
        )
}

fn render_findings(snapshot: &SnapshotData) -> Div {
    let section = section("Findings", "ranked, most actionable first");
    if snapshot.findings.is_empty() {
        return section.child(
            div()
                .p_3()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child("No findings were derived for this recording."),
        );
    }
    section.children(snapshot.findings.iter().map(|finding| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(severity_badge(finding.severity))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED_TEXT))
                            .child(finding.resource.clone()),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .child(finding.finding.clone()),
                    )
                    .child(div().flex_1())
                    .child(scope_tag(&finding.quality)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(finding.evidence.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child(format!("Next: {}", finding.recommendation)),
            )
    }))
}

fn render_resource_charts(snapshot: &SnapshotData, selected: &str) -> Div {
    let Some(resource) = snapshot
        .resources
        .iter()
        .find(|resource| resource.resource == selected)
    else {
        return div();
    };
    section(
        format!("{} over time", display_resource(selected)),
        "rates derived from counters · click another resource to switch",
    )
    .child(
        div()
            .p_2()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(resource.charts.iter().map(chart_panel)),
    )
}

fn chart_panel(chart: &SnapshotChart) -> Div {
    let stats = format!(
        "max {} · last {} · {}",
        format_chart_value(chart.max_value, &chart.unit),
        format_chart_value(
            chart.series.iter().map(|series| series.last).sum::<f64>(),
            &chart.unit
        ),
        chart.scope.replace('_', " "),
    );
    let paint_chart = chart.clone();
    div()
        .w(px(392.0))
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(WORKSPACE))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .px_2()
                .py_1()
                .gap(px(1.0))
                .bg(rgb(CHROME))
                .text_xs()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(chart.metric.replace('_', " ")),
                        )
                        .child(category_tag(chart.category)),
                )
                .child(div().truncate().text_color(rgb(MUTED_TEXT)).child(stats)),
        )
        .child(
            div().h(px(96.0)).child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| paint_series(bounds, &paint_chart, window),
                )
                .size_full(),
            ),
        )
        .when(chart.series.len() > 1, |element| {
            element.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .children(
                        chart
                            .series
                            .iter()
                            .take(8)
                            .enumerate()
                            .map(|(index, series)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .w(px(8.0))
                                            .h(px(3.0))
                                            .bg(rgb(SERIES_PALETTE[index % SERIES_PALETTE.len()])),
                                    )
                                    .child(series.id.clone())
                            }),
                    )
                    .when(chart.series.len() > 8, |element| {
                        element.child(format!("+{} more", chart.series.len() - 8))
                    }),
            )
        })
        .child(div().h(px(2.0)))
}

fn paint_series(bounds: Bounds<Pixels>, chart: &SnapshotChart, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    window.paint_quad(fill(bounds, rgb(WORKSPACE)));
    for fraction in [0.25, 0.5, 0.75] {
        let y = bounds.origin.y + px(height * fraction);
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x, y),
                gpui::size(bounds.size.width, px(1.0)),
            ),
            rgb(CHROME),
        ));
    }

    let max_time = chart
        .series
        .iter()
        .filter_map(|series| series.points.last())
        .map(|point| point.0)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let max_value = chart.max_value.max(f64::EPSILON);
    let inset = 3.0_f32;

    for (index, series) in chart.series.iter().enumerate() {
        let color = SERIES_PALETTE[index % SERIES_PALETTE.len()];
        let mut path = PathBuilder::stroke(px(1.5));
        let mut started = false;
        for (time, value) in &series.points {
            let x = bounds.origin.x + px(inset + (*time / max_time) as f32 * (width - 2.0 * inset));
            let y = bounds.origin.y
                + px(height - inset - ((*value / max_value) as f32 * (height - 2.0 * inset)));
            if started {
                path.line_to(point(x, y));
            } else {
                path.move_to(point(x, y));
                started = true;
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(color));
        }
    }
}

fn render_collectors(snapshot: &SnapshotData) -> Div {
    let section = section("Collectors", "coverage and data quality");
    if snapshot.collectors.is_empty() {
        return section.child(
            div()
                .p_3()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child("No collector records were persisted."),
        );
    }
    section.children(snapshot.collectors.iter().map(|collector| {
        let available = collector.status == "available";
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .child(
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(rgb(if available { 0x9ece6a } else { ERROR })),
            )
            .child(
                div()
                    .w(px(120.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(collector.name.clone()),
            )
            .child(
                div()
                    .w(px(130.0))
                    .text_color(rgb(if available { TEXT } else { ERROR }))
                    .child(collector.status.clone()),
            )
            .child(scope_tag(&collector.quality))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(rgb(MUTED_TEXT))
                    .child(collector.message.clone()),
            )
    }))
}

fn section(title: impl Into<String>, subtitle: impl Into<String>) -> Div {
    div()
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .overflow_hidden()
        .child(
            div()
                .h(px(30.0))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .border_b_1()
                .border_color(rgb(BORDER))
                .bg(rgb(CHROME))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.into()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child(subtitle.into()),
                ),
        )
}

fn severity_color(severity: Severity) -> u32 {
    match severity {
        Severity::High => ERROR,
        Severity::Medium => ACCENT,
        Severity::Info => MUTED_TEXT,
    }
}

fn severity_dot(severity: Severity) -> Div {
    div()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .bg(rgb(severity_color(severity)))
}

fn severity_badge(severity: Severity) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(rgb(HOVER))
        .text_color(rgb(severity_color(severity)))
        .child(severity.label())
}

fn scope_tag(scope: &str) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .bg(rgb(HOVER))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .whitespace_nowrap()
        .child(scope.replace('_', " "))
}

fn category_tag(category: UseCategory) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .bg(rgb(HOVER))
        .text_xs()
        .text_color(rgb(MUTED_TEXT))
        .child(category.title())
}

fn display_resource(resource: &str) -> &str {
    match resource {
        "cpu" => "CPU",
        "memory" => "Memory",
        "disk" => "Disk",
        "io" => "I/O",
        "network" => "Network",
        other => other,
    }
}

fn format_seconds(seconds: f64) -> String {
    if seconds >= 60.0 {
        format!("{:.0}m {:.0}s", (seconds / 60.0).floor(), seconds % 60.0)
    } else {
        format!("{seconds:.1}s")
    }
}

fn format_chart_value(value: f64, unit: &str) -> String {
    match unit {
        "bytes/s" => format!("{}/s", format_bytes(value)),
        "cores" => format!("{value:.2} cores"),
        "%" | "percent" => format!("{value:.1}%"),
        "bits_per_second" => format!("{:.1} Gbit/s", value / 1e9),
        "bytes" => format_bytes(value),
        unit if unit.ends_with("/s") => format!("{}/s", format_count(value)),
        _ => format_count(value),
    }
}
