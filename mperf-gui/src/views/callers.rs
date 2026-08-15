use std::collections::BTreeSet;

use std::rc::Rc;

use gpui::{Context, Div, FontWeight, SharedString, div, prelude::*, px, rgb, uniform_list};

use crate::{
    MperfGui,
    profile::{CounterMetric, ProfileFrame},
    profile_analysis::{
        FunctionAnalysis, FunctionDetails, FunctionRelation, FunctionStat, SampleFilter,
    },
    theme::{
        ACCENT, BORDER, CHROME, ERROR, HOVER, MUTED_TEXT, SELECTION_MUTED, SURFACE, TEXT, WORKSPACE,
    },
};

const HEADER_HEIGHT: f32 = 30.0;
const LIST_HEADER_HEIGHT: f32 = 24.0;
const FUNCTION_ROW_HEIGHT: f32 = 40.0;
const RELATION_ROW_HEIGHT: f32 = 26.0;
const RANKED_WIDTH: f32 = 350.0;
const RANK_WIDTH: f32 = 42.0;
const SAMPLES_WIDTH: f32 = 92.0;
const PERCENT_WIDTH: f32 = 76.0;

impl MperfGui {
    pub(crate) fn render_callers_and_callees_workspace(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = self.model.as_ref() else {
            return callers_message(
                "Open a recording to inspect callers and callees.",
                false,
                false,
                cx,
            );
        };
        if let Some(error) = model.profile.error.as_ref() {
            return callers_message(error.clone(), true, false, cx);
        }
        if model.profile.samples.is_empty() {
            return callers_message(
                "This recording has no timestamped call-stack samples.",
                false,
                false,
                cx,
            );
        }

        let filter = self.profile_sample_filter();
        let filter_active = sample_filter_is_active(&filter);
        let Some(analysis) = self.cached_function_analysis() else {
            return callers_message(
                "This recording has no timestamped call-stack samples.",
                false,
                false,
                cx,
            );
        };
        if analysis.total_samples == 0 {
            return callers_message(
                "No call-stack samples match the active filter.",
                false,
                filter_active,
                cx,
            );
        }
        if analysis.functions.is_empty() {
            return callers_message(
                "The matching samples contain no resolved functions.",
                false,
                filter_active,
                cx,
            );
        }

        let Some(selected_frame) =
            selected_frame_id(&analysis, &filter, self.selected_function.as_deref())
                .or_else(|| analysis.functions.first().map(|function| function.frame_id))
        else {
            return callers_message(
                "The matching samples contain no selectable functions.",
                false,
                filter_active,
                cx,
            );
        };
        let Some(details) = analysis.details(selected_frame) else {
            return callers_message(
                "The selected function is not present in the active sample set.",
                false,
                filter_active,
                cx,
            );
        };
        let filter_summary = filter_summary(
            &filter,
            &model.profile.frames,
            &model.profile.counter_metrics,
        );

        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(rgb(WORKSPACE))
            .child(self.render_callers_header(&analysis, filter_summary, filter_active, cx))
            .child(
                div()
                    .min_h(px(0.0))
                    .flex_1()
                    .flex()
                    .child(self.render_ranked_functions(analysis.clone(), selected_frame, cx))
                    .child(self.render_sandwich(details, cx)),
            )
    }

    fn render_callers_header(
        &self,
        analysis: &FunctionAnalysis,
        filter_summary: Option<String>,
        filter_active: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = format!(
            "{} samples · {} ranked functions",
            analysis.total_samples,
            analysis.functions.len()
        );

        div()
            .h(px(HEADER_HEIGHT))
            .min_h(px(HEADER_HEIGHT))
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
                    .text_color(rgb(TEXT))
                    .child("Callers & Callees"),
            )
            .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(status))
            .when_some(filter_summary, |element, summary| {
                element.child(
                    div()
                        .min_w_0()
                        .max_w(px(430.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .rounded_sm()
                        .bg(rgb(CHROME))
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child(summary),
                )
            })
            .child(div().flex_1())
            .when(filter_active, |element| {
                element.child(
                    div()
                        .id("callers-clear-filter")
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .px_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.clear_global_filter();
                            cx.notify();
                        }))
                        .child("Clear Filter"),
                )
            })
    }

    fn render_ranked_functions(
        &self,
        analysis: Rc<FunctionAnalysis>,
        selected_frame: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .w(px(RANKED_WIDTH))
            .min_w(px(320.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(LIST_HEADER_HEIGHT))
                    .min_h(px(LIST_HEADER_HEIGHT))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(CHROME))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(MUTED_TEXT))
                    .child(
                        div()
                            .w(px(RANK_WIDTH))
                            .min_w(px(RANK_WIDTH))
                            .whitespace_nowrap()
                            .child("RANK"),
                    )
                    .child(div().flex_1().child("FUNCTION"))
                    .child(div().w(px(68.0)).text_right().child("INCL %")),
            )
            .child({
                let entity = cx.entity();
                uniform_list(
                    "callers-ranked-functions-scroll",
                    analysis.functions.len(),
                    move |range, _, _| {
                        range
                            .map(|index| {
                                let function = &analysis.functions[index];
                                ranked_function_row(
                                    index,
                                    function,
                                    function.frame_id == selected_frame,
                                    entity.clone(),
                                )
                            })
                            .collect()
                    },
                )
                .min_h(px(0.0))
                .flex_1()
            })
    }
}

fn ranked_function_row(
    index: usize,
    function: &FunctionStat,
    selected: bool,
    entity: gpui::Entity<MperfGui>,
) -> impl IntoElement + use<> {
    let frame_id = function.frame_id;
    let label = function.label.clone();
    let samples = format!(
        "{} self · {} incl",
        function.self_samples, function.inclusive_samples
    );

    div()
        .id(SharedString::from(format!(
            "callers-ranked-function-{index}"
        )))
        // uniform_list lays rows out as roots, so they must claim the width.
        .w_full()
        .h(px(FUNCTION_ROW_HEIGHT))
        .min_h(px(FUNCTION_ROW_HEIGHT))
        .flex()
        .items_center()
        .overflow_hidden()
        .border_b_1()
        .border_color(rgb(BORDER))
        .cursor_pointer()
        .when(selected, |element| {
            element
                .bg(rgb(SELECTION_MUTED))
                .border_l_2()
                .border_color(rgb(ACCENT))
        })
        .when(!selected, |element| {
            element.hover(|element| element.bg(rgb(HOVER)))
        })
        .on_click(move |_, _, cx| {
            entity.update(cx, |view, cx| {
                view.select_profile_function(frame_id);
                cx.notify();
            })
        })
        .child(
            div()
                .w(px(RANK_WIDTH))
                .min_w(px(RANK_WIDTH))
                .pl_3()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(format!("{}", index + 1)),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().line_height(px(18.0)).text_sm().child(label))
                .child(
                    div()
                        .line_height(px(16.0))
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child(samples),
                ),
        )
        .child(
            div()
                .w(px(72.0))
                .pr_3()
                .text_right()
                .text_sm()
                .child(format_percent(function.inclusive_fraction)),
        )
}

impl MperfGui {
    fn render_sandwich(&self, details: FunctionDetails, cx: &mut Context<Self>) -> Div {
        let current = details.function;
        div()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex_1()
            .flex()
            .flex_col()
            .child(relation_section_header(
                "CALLERS",
                "Functions that lead into the selected function",
                details.callers.len(),
            ))
            .child(self.render_relations(
                "callers-callers-scroll",
                details.callers,
                "No sampled caller: this function is a root in the selected stacks.",
                "↓",
                cx,
            ))
            .child(current_function(&current))
            .child(relation_section_header(
                "CALLEES",
                "Functions called directly by the selected function",
                details.callees.len(),
            ))
            .child(self.render_relations(
                "callers-callees-scroll",
                details.callees,
                "No sampled callees: this function is a leaf in the selected stacks.",
                "↓",
                cx,
            ))
    }

    fn render_relations(
        &self,
        id: &'static str,
        relations: Vec<FunctionRelation>,
        empty: &'static str,
        direction: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .min_h(px(72.0))
            .flex_1()
            .overflow_y_scroll()
            .when(relations.is_empty(), |element| {
                element.child(
                    div()
                        .h_full()
                        .min_h(px(72.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child(empty),
                )
            })
            .children(relations.into_iter().enumerate().map(|(index, relation)| {
                self.render_relation_row(id, index, relation, direction, cx)
            }))
    }

    fn render_relation_row(
        &self,
        section: &'static str,
        index: usize,
        relation: FunctionRelation,
        direction: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let frame_id = relation.frame_id;
        div()
            .id(SharedString::from(format!("{section}-relation-{index}")))
            .h(px(RELATION_ROW_HEIGHT))
            .min_h(px(RELATION_ROW_HEIGHT))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .cursor_pointer()
            .hover(|element| element.bg(rgb(HOVER)))
            .on_click(cx.listener(move |view, _, _, cx| {
                view.select_profile_function(frame_id);
                cx.notify();
            }))
            .child(
                div()
                    .w(px(34.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(ACCENT))
                    .child(direction),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_sm()
                    .child(relation.label),
            )
            .child(numeric_cell(relation.samples.to_string(), SAMPLES_WIDTH))
            .child(numeric_cell(
                format_percent(relation.fraction_of_function),
                PERCENT_WIDTH,
            ))
    }
}

fn selected_frame_id(
    analysis: &FunctionAnalysis,
    filter: &SampleFilter,
    selected_function: Option<&str>,
) -> Option<usize> {
    if let Some(frame_ids) = filter.frame_ids.as_ref()
        && let Some(frame_id) = analysis
            .functions
            .iter()
            .find(|function| frame_ids.contains(&function.frame_id))
            .map(|function| function.frame_id)
    {
        return Some(frame_id);
    }
    selected_function.and_then(|selected| {
        analysis
            .functions
            .iter()
            .find(|function| names_match(&function.label, selected))
            .map(|function| function.frame_id)
    })
}

fn sample_filter_is_active(filter: &SampleFilter) -> bool {
    filter.range.is_some()
        || filter.process_id.is_some()
        || filter.thread_id.is_some()
        || filter.cpu.is_some()
        || filter.required_counter.is_some()
        || filter.frame_ids.is_some()
}

fn filter_summary(
    filter: &SampleFilter,
    frames: &[ProfileFrame],
    counters: &[CounterMetric],
) -> Option<String> {
    let mut parts = Vec::new();
    if filter.range.is_some() {
        parts.push("time range".to_string());
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
        let label = counters
            .get(counter_index)
            .map(|counter| counter.label.as_str())
            .unwrap_or("counter");
        parts.push(format!("{label} present"));
    }
    if let Some(frame_ids) = filter.frame_ids.as_ref() {
        parts.push(frame_filter_label(frame_ids, frames));
    }
    (!parts.is_empty()).then(|| format!("Filtered by {}", parts.join(" · ")))
}

fn frame_filter_label(frame_ids: &BTreeSet<usize>, frames: &[ProfileFrame]) -> String {
    let names = frame_ids
        .iter()
        .filter_map(|frame_id| frames.get(*frame_id))
        .map(|frame| frame.name.as_str())
        .take(2)
        .collect::<Vec<_>>();
    match names.as_slice() {
        [] => "function".to_string(),
        [name] => format!("function {name}"),
        [first, second] if frame_ids.len() == 2 => {
            format!("functions {first}, {second}")
        }
        [first, second] => {
            format!(
                "functions {first}, {second} +{}",
                frame_ids.len().saturating_sub(2)
            )
        }
        _ => unreachable!(),
    }
}

fn names_match(left: &str, right: &str) -> bool {
    left == right || left.strip_prefix('_') == Some(right) || right.strip_prefix('_') == Some(left)
}

fn relation_section_header(title: &'static str, detail: &'static str, count: usize) -> Div {
    div()
        .h(px(LIST_HEADER_HEIGHT))
        .min_h(px(LIST_HEADER_HEIGHT))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CHROME))
        .text_xs()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(format!("{title} · {count}")),
        )
        .child(div().text_color(rgb(MUTED_TEXT)).child(detail))
        .child(div().flex_1())
        .child(
            div()
                .w(px(SAMPLES_WIDTH))
                .text_right()
                .text_color(rgb(MUTED_TEXT))
                .child("SAMPLES"),
        )
        .child(
            div()
                .w(px(PERCENT_WIDTH))
                .text_right()
                .text_color(rgb(MUTED_TEXT))
                .child("OF CURRENT"),
        )
}

fn current_function(function: &FunctionStat) -> Div {
    div()
        .min_h(px(76.0))
        .flex()
        .items_center()
        .px_3()
        .gap_3()
        .border_t_1()
        .border_b_1()
        .border_color(rgb(ACCENT))
        .bg(rgb(SELECTION_MUTED))
        .child(
            div()
                .w(px(34.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(ACCENT))
                .font_weight(FontWeight::BOLD)
                .child("◆"),
        )
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
                        .text_color(rgb(TEXT))
                        .child(function.label.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child("Selected function"),
                ),
        )
        .child(metric_block(
            "SELF",
            function.self_samples,
            function.self_fraction,
        ))
        .child(metric_block(
            "INCLUSIVE",
            function.inclusive_samples,
            function.inclusive_fraction,
        ))
}

fn metric_block(label: &'static str, samples: u64, fraction: f64) -> Div {
    div()
        .w(px(126.0))
        .flex()
        .flex_col()
        .items_end()
        .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(label))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{samples} · {}", format_percent(fraction))),
        )
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
        .text_sm()
        .child(value)
}

fn format_percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

fn callers_message(
    message: impl Into<SharedString>,
    error: bool,
    clear_filter: bool,
    cx: &mut Context<MperfGui>,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgb(WORKSPACE))
        .child(
            div()
                .h(px(HEADER_HEIGHT))
                .min_h(px(HEADER_HEIGHT))
                .flex()
                .items_center()
                .px_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Callers & Callees")
                .child(div().flex_1())
                .when(clear_filter, |element| {
                    element.child(
                        div()
                            .id("callers-empty-clear-filter")
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .px_1()
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(ACCENT))
                            .hover(|element| element.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.clear_global_filter();
                                cx.notify();
                            }))
                            .child("Clear Filter"),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_sm()
                .text_color(rgb(if error { ERROR } else { MUTED_TEXT }))
                .child(message.into()),
        )
}
