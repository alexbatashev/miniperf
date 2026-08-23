//! Resources view: the USE-method snapshot as one card per resource —
//! utilization / saturation / errors rows with meters and sparklines — plus
//! the ranked findings the collector produced.

use std::sync::Arc;

use gpui::{Context, FontWeight, Hsla, canvas, div, fill, prelude::*, px};

use super::ShellView;
use super::session::ShellSession;
use crate::charts::paint_area_series;
use crate::snapshot::{
    ResourceUse, Severity, SnapshotChart, SnapshotData, SnapshotFinding, SummaryMetric, UseCategory,
};
use crate::ui::{self, ActiveTheme, Icon, Theme, badge, empty_state};

const SPARK_H: f32 = 32.0;

pub fn render(session: &Arc<ShellSession>, cx: &mut Context<ShellView>) -> gpui::AnyElement {
    let theme = cx.theme().clone();
    let Some(snapshot) = session.snapshot.as_ref() else {
        return empty_state(Icon::Gauge, "No resource snapshot in this recording")
            .into_any_element();
    };

    div()
        .id("resources")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(8.0))
        .child(
            div().flex().flex_wrap().gap(px(8.0)).children(
                snapshot
                    .resources
                    .iter()
                    .map(|resource| resource_card(resource, &theme, cx)),
            ),
        )
        .when(!snapshot.findings.is_empty(), |el| {
            el.child(ui::section_caption("findings · ranked", cx))
                .child(
                    div().flex().flex_col().gap(px(6.0)).children(
                        snapshot
                            .findings
                            .iter()
                            .map(|finding| finding_card(finding, &theme)),
                    ),
                )
        })
        .when(!snapshot.collectors.is_empty(), |el| {
            el.child(collectors_card(snapshot, &theme))
        })
        .into_any_element()
}

fn resource_card(
    resource: &ResourceUse,
    theme: &Theme,
    cx: &mut Context<ShellView>,
) -> gpui::AnyElement {
    let mut card = ui::viz_card(resource.resource.to_uppercase());
    if let Some(headline) = resource.headline.clone() {
        card = card.action(
            div()
                .text_size(px(10.0))
                .text_color(theme.muted_foreground)
                .child(headline),
        );
    }

    for (index, category) in UseCategory::ALL.into_iter().enumerate() {
        let metrics = &resource.summaries[index];
        if metrics.is_empty() {
            continue;
        }
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(ui::section_caption(category.title(), cx))
                .children(
                    metrics
                        .iter()
                        .map(|metric| metric_row(metric, category, theme)),
                ),
        );
    }

    if let Some(chart) = spark_chart(resource) {
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} · {}",
                            chart.metric.replace('_', " "),
                            chart.unit
                        )),
                )
                .child(sparkline(chart, theme.viz.series[0], theme)),
        );
    }

    div()
        .flex_1()
        .min_w(px(240.0))
        .child(card)
        .into_any_element()
}

fn metric_row(metric: &SummaryMetric, category: UseCategory, theme: &Theme) -> gpui::AnyElement {
    let fraction = metric.fraction.unwrap_or(0.0) as f32;
    div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.0))
                .text_size(px(10.5))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(metric.metric.replace('_', " ")),
                )
                .child(div().flex_none().child(metric.value.clone()))
                .when(!metric.scope.is_empty(), |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(theme.muted_foreground)
                            .child(metric.scope.replace('_', " ")),
                    )
                }),
        )
        .when(metric.fraction.is_some(), |el| {
            el.child(ui::meter(fraction).color(meter_color(category, fraction, theme)))
        })
        .into_any_element()
}

/// Utilization reads as load, saturation and errors read as damage — so they
/// escalate through the status ramp at very different thresholds.
fn meter_color(category: UseCategory, fraction: f32, theme: &Theme) -> Hsla {
    match category {
        UseCategory::Errors => {
            if fraction > 0.0 {
                theme.viz.status_critical
            } else {
                theme.viz.status_good
            }
        }
        UseCategory::Saturation => match fraction {
            value if value >= 0.30 => theme.viz.status_critical,
            value if value >= 0.06 => theme.viz.status_serious,
            _ => theme.viz.status_good,
        },
        UseCategory::Utilization => match fraction {
            value if value >= 0.85 => theme.viz.status_serious,
            value if value >= 0.70 => theme.viz.status_warn,
            _ => theme.viz.series[0],
        },
    }
}

/// The card's one sparkline: engine occupancy for a device that reports it,
/// else its clock; otherwise the busiest utilization series, falling back to
/// whatever the resource does have.
fn spark_chart(resource: &ResourceUse) -> Option<&SnapshotChart> {
    if matches!(resource.resource.as_str(), "gpu" | "npu") {
        let named = |metric: &str| {
            resource
                .charts
                .iter()
                .find(|chart| chart.metric == metric && chart.max_value > 0.0)
        };
        if let Some(chart) = named("busy").or_else(|| named("frequency")) {
            return Some(chart);
        }
    }
    resource
        .charts
        .iter()
        .filter(|chart| chart.category == UseCategory::Utilization)
        .chain(resource.charts.iter())
        .find(|chart| chart.max_value > 0.0)
}

fn sparkline(chart: &SnapshotChart, color: Hsla, theme: &Theme) -> impl IntoElement + use<> {
    let surface = theme.viz.surface;
    let max = chart.max_value;
    let series: Vec<Vec<(f64, f64)>> = chart
        .series
        .iter()
        .map(|series| series.points.clone())
        .collect();
    let (first, last) = series
        .iter()
        .flatten()
        .fold((f64::MAX, f64::MIN), |(first, last), point| {
            (first.min(point.0), last.max(point.0))
        });

    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, surface));
            let span = (last - first).max(1e-9);
            let left = f32::from(bounds.left());
            let width = f32::from(bounds.size.width);
            for points in &series {
                let mapped: Vec<(f32, Option<f64>)> = points
                    .iter()
                    .map(|(time, value)| {
                        (left + ((time - first) / span) as f32 * width, Some(*value))
                    })
                    .collect();
                paint_area_series(
                    f32::from(bounds.top()) + 1.0,
                    f32::from(bounds.size.height) - 2.0,
                    &mapped,
                    max,
                    color,
                    window,
                );
            }
        },
    )
    .w_full()
    .h(px(SPARK_H))
}

fn finding_card(finding: &SnapshotFinding, theme: &Theme) -> gpui::AnyElement {
    let detail = |label: &'static str, text: String| {
        div()
            .flex()
            .gap(px(4.0))
            .text_size(px(11.0))
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .child(div().flex_1().min_w(px(0.0)).child(text))
    };

    ui::viz_panel()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    badge(finding.severity.label().to_uppercase())
                        .tint(severity_color(finding.severity, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(finding.finding.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child(finding.resource.to_uppercase()),
                ),
        )
        .child(detail("Evidence:", finding.evidence.clone()))
        .child(detail("Try:", finding.recommendation.clone()))
        .when(!finding.quality.is_empty(), |card| {
            card.child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme.muted_foreground)
                    .child(format!("quality: {}", finding.quality)),
            )
        })
        .into_any_element()
}

pub(super) fn severity_color(severity: Severity, theme: &Theme) -> Hsla {
    match severity {
        Severity::High => theme.viz.status_critical,
        Severity::Medium => theme.viz.status_serious,
        Severity::Info => theme.viz.series[0],
    }
}

fn collectors_card(snapshot: &SnapshotData, theme: &Theme) -> gpui::AnyElement {
    ui::viz_card("collectors")
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(snapshot.collectors.iter().map(|collector| {
                    let color = match collector.status.as_str() {
                        "ok" => theme.viz.status_good,
                        "partial" => theme.viz.status_warn,
                        _ => theme.viz.status_critical,
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .rounded(px(3.0))
                        .bg(theme.muted)
                        .px(px(6.0))
                        .py(px(2.0))
                        .text_size(px(10.0))
                        .child(div().size(px(6.0)).rounded_full().bg(color))
                        .child(collector.name.clone())
                        .when(!collector.message.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(collector.message.clone()),
                            )
                        })
                })),
        )
        .into_any_element()
}
