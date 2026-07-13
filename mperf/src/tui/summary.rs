use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use mperf_data::RecordInfo;
use num_format::Locale;
use num_format::ToFormattedString;
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    widgets::{Block, Paragraph, Row, Table, Widget, Wrap},
};
use sqlite::Connection;

#[derive(Clone)]
pub struct SummaryTab {
    record_info: RecordInfo,
    connection: Arc<Mutex<Connection>>,
    stat: Arc<RwLock<Stat>>,
    load_started: Arc<AtomicBool>,
    load_error: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Stat {
    cycles: u64,
    instructions: u64,
    branch_instructions: Option<u64>,
    branch_misses: Option<u64>,
    cache_references: Option<u64>,
    cache_misses: Option<u64>,
    stalled_cycles_frontend: Option<u64>,
    stalled_cycles_backend: Option<u64>,
    initialized: bool,
}

impl SummaryTab {
    pub fn new(record_info: RecordInfo, connection: Arc<Mutex<Connection>>) -> Self {
        SummaryTab {
            record_info,
            connection,
            stat: Arc::new(RwLock::new(Stat::default())),
            load_started: Arc::new(AtomicBool::new(false)),
            load_error: Arc::new(RwLock::new(None)),
        }
    }

    pub fn run(&self) {
        if self
            .load_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let this = self.clone();
        tokio::spawn(this.fetch_data());
    }

    async fn fetch_data(self) {
        let conn = self.connection.lock();
        let result: Result<Stat, String> = (|| {
            let available_columns: HashSet<String> = conn
                .prepare("PRAGMA table_info(pmu_counters);")
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|row| {
                    let row = row.map_err(|error| error.to_string())?;
                    Ok(row.read::<&str, _>("name").to_string())
                })
                .collect::<Result<_, String>>()?;

            let has_branch = available_columns.contains("pmu_branch_instructions")
                && available_columns.contains("pmu_branch_misses");
            let has_cache = available_columns.contains("pmu_llc_references")
                && available_columns.contains("pmu_llc_misses");
            let has_stalled = available_columns.contains("pmu_stalled_cycles_frontend")
                && available_columns.contains("pmu_stalled_cycles_backend");

            let mut select_parts = vec![
                "SUM(pmu_cycles) AS pmu_cycles".to_string(),
                "SUM(pmu_instructions) AS pmu_instructions".to_string(),
            ];

            if has_branch {
                select_parts.push(
                "CAST(SUM(pmu_branch_instructions * 1.0 / confidence) AS INTEGER) AS pmu_branch_instructions"
                    .to_string(),
            );
                select_parts.push(
                "CAST(SUM(pmu_branch_misses * 1.0 / confidence) AS INTEGER) AS pmu_branch_misses"
                    .to_string(),
            );
            } else {
                select_parts.push("0 AS pmu_branch_instructions".to_string());
                select_parts.push("0 AS pmu_branch_misses".to_string());
            }

            if has_cache {
                select_parts.push(
                "CAST(SUM(pmu_llc_references * 1.0 / confidence) AS INTEGER) AS pmu_llc_references"
                    .to_string(),
            );
                select_parts.push(
                    "CAST(SUM(pmu_llc_misses * 1.0 / confidence) AS INTEGER) AS pmu_llc_misses"
                        .to_string(),
                );
            } else {
                select_parts.push("0 AS pmu_llc_references".to_string());
                select_parts.push("0 AS pmu_llc_misses".to_string());
            }

            if has_stalled {
                select_parts.push(
                "CAST(SUM(pmu_stalled_cycles_frontend * 1.0 / confidence) AS INTEGER) AS pmu_stalled_cycles_frontend"
                    .to_string(),
            );
                select_parts.push(
                "CAST(SUM(pmu_stalled_cycles_backend * 1.0 / confidence) AS INTEGER) AS pmu_stalled_cycles_backend"
                    .to_string(),
            );
            } else {
                select_parts.push("0 AS pmu_stalled_cycles_frontend".to_string());
                select_parts.push("0 AS pmu_stalled_cycles_backend".to_string());
            }

            let query = format!("SELECT {} FROM pmu_counters;", select_parts.join(",\n"));
            let mut rows = conn
                .prepare(&query)
                .map_err(|error| error.to_string())?
                .into_iter();
            let row = rows
                .next()
                .ok_or_else(|| "summary query returned no rows".to_string())?
                .map_err(|error| error.to_string())?;

            let read = |name| {
                row.try_read::<Option<i64>, _>(name)
                    .map(|value| value.unwrap_or_default() as u64)
                    .map_err(|error| error.to_string())
            };
            Ok(Stat {
                cycles: read("pmu_cycles")?,
                instructions: read("pmu_instructions")?,
                branch_instructions: has_branch
                    .then(|| read("pmu_branch_instructions"))
                    .transpose()?,
                branch_misses: has_branch.then(|| read("pmu_branch_misses")).transpose()?,
                cache_references: has_cache.then(|| read("pmu_llc_references")).transpose()?,
                cache_misses: has_cache.then(|| read("pmu_llc_misses")).transpose()?,
                stalled_cycles_frontend: has_stalled
                    .then(|| read("pmu_stalled_cycles_frontend"))
                    .transpose()?,
                stalled_cycles_backend: has_stalled
                    .then(|| read("pmu_stalled_cycles_backend"))
                    .transpose()?,
                initialized: true,
            })
        })();
        drop(conn);

        match result {
            Ok(stat) => *self.stat.write() = stat,
            Err(error) => {
                *self.load_error.write() = Some(format!("Could not load summary data:\n\n{error}"));
            }
        }
    }
}

impl Widget for SummaryTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let horizontal = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]);
        let [summary_area, _right_area] = horizontal.areas(area);

        let vertical = Layout::vertical_margin(
            Layout::vertical([Constraint::Fill(3), Constraint::Fill(1)]),
            1,
        );
        let [stat_area, info_area] = vertical.areas(summary_area);

        let block = Block::bordered().title("Counters stats");
        block.render(stat_area, buf);

        let vertical = Layout::horizontal_margin(
            Layout::vertical_margin(Layout::vertical([Constraint::Fill(1)]), 1),
            2,
        );
        let [stat_table_area] = vertical.areas(stat_area);

        {
            let stat = self.stat.read();

            if let Some(error) = self.load_error.read().clone() {
                Paragraph::new(error)
                    .wrap(Wrap { trim: true })
                    .render(stat_table_area, buf);
            } else if !stat.initialized {
                let counter = 0;
                let pb = ratatui::widgets::Gauge::default()
                    .block(Block::bordered().title("Loading data..."))
                    .gauge_style(Style::new().white().on_black().italic())
                    .percent(counter);
                pb.render(stat_table_area, buf);
            } else {
                let ipc = if stat.cycles > 0 {
                    format!("{:.2}", stat.instructions as f64 / stat.cycles as f64)
                } else {
                    "N/A".to_string()
                };

                let branch_instruction_count = format_optional_count(stat.branch_instructions);
                let branch_per_cycle = match (stat.branch_instructions, stat.cycles) {
                    (Some(branch_instr), cycles) if cycles > 0 => {
                        format!("{:.2} per cycle", branch_instr as f64 / cycles as f64)
                    }
                    _ => "N/A".to_string(),
                };

                let branch_miss_count = format_optional_count(stat.branch_misses);
                let branch_miss_pct = match (stat.branch_misses, stat.branch_instructions) {
                    (Some(misses), Some(instructions)) if instructions > 0 => {
                        format!("{:.2}%", misses as f64 / instructions as f64 * 100_f64)
                    }
                    _ => "N/A".to_string(),
                };

                let branch_mpki = match (stat.branch_misses, stat.instructions) {
                    (Some(misses), instructions) if instructions > 0 => {
                        format!("{:.2}", misses as f64 / instructions as f64 * 1000.0)
                    }
                    _ => "N/A".to_string(),
                };

                let cache_ref_count = format_optional_count(stat.cache_references);
                let cache_miss_count = format_optional_count(stat.cache_misses);
                let cache_miss_pct = match (stat.cache_misses, stat.cache_references) {
                    (Some(misses), Some(references)) if misses + references > 0 => {
                        format!(
                            "{:.2}%",
                            misses as f64 / (misses + references) as f64 * 100_f64
                        )
                    }
                    _ => "N/A".to_string(),
                };
                let cache_mpki = match (stat.cache_misses, stat.instructions) {
                    (Some(misses), instructions) if instructions > 0 => {
                        format!("{:.2}", misses as f64 / instructions as f64 * 1000.0)
                    }
                    _ => "N/A".to_string(),
                };
                let stalled_backend_count = format_optional_count(stat.stalled_cycles_backend);
                let stalled_backend_pct =
                    format_optional_ratio(stat.stalled_cycles_backend, stat.cycles);
                let stalled_frontend_count = format_optional_count(stat.stalled_cycles_frontend);
                let stalled_frontend_pct =
                    format_optional_ratio(stat.stalled_cycles_frontend, stat.cycles);

                let rows = [
                    Row::new([
                        "Cycles".to_string(),
                        stat.cycles.to_formatted_string(&Locale::en),
                        "".to_string(),
                    ]),
                    Row::new([
                        "Instructions".to_string(),
                        stat.instructions.to_formatted_string(&Locale::en),
                        "".to_string(),
                    ]),
                    Row::new(["IPC".to_string(), ipc, "".to_string()]),
                    Row::new([
                        "Branch instructions".to_string(),
                        branch_instruction_count,
                        branch_per_cycle,
                    ]),
                    Row::new([
                        "Branch misses".to_string(),
                        branch_miss_count,
                        branch_miss_pct,
                    ]),
                    Row::new(["Branch MPKI".to_string(), branch_mpki, "".to_string()]),
                    Row::new([
                        "Last level cache references".to_string(),
                        cache_ref_count,
                        "".to_string(),
                    ]),
                    Row::new([
                        "Last level cache misses".to_string(),
                        cache_miss_count,
                        cache_miss_pct,
                    ]),
                    Row::new(["Cache MPKI".to_string(), cache_mpki, "".to_string()]),
                    Row::new([
                        "Stalled cycles backend".to_string(),
                        stalled_backend_count,
                        stalled_backend_pct,
                    ]),
                    Row::new([
                        "Stalled cycles frontend".to_string(),
                        stalled_frontend_count,
                        stalled_frontend_pct,
                    ]),
                ];
                let widths = [
                    Constraint::Percentage(60),
                    Constraint::Percentage(20),
                    Constraint::Percentage(20),
                ];
                let stat_table = Table::new(rows, widths).column_spacing(1);
                stat_table.render(stat_table_area, buf);
            }
        }

        let block = Block::bordered().title("Result info");
        block.render(info_area, buf);

        let command = self
            .record_info
            .command
            .unwrap_or(vec!["".to_string()])
            .join(" ");

        let rows = [
            Row::new(["Scenario", self.record_info.scenario.name()]),
            Row::new(["Command", command.as_str()]),
            Row::new(["CPU family", self.record_info.cpu_model.as_str()]),
            Row::new(["CPU vendor", self.record_info.cpu_vendor.as_str()]),
        ];
        let widths = [Constraint::Percentage(20), Constraint::Percentage(80)];

        let vertical = Layout::horizontal_margin(
            Layout::vertical_margin(Layout::vertical([Constraint::Fill(1)]), 1),
            2,
        );
        let [info_table_area] = vertical.areas(info_area);

        let info_table = Table::new(rows, widths).column_spacing(1);
        info_table.render(info_table_area, buf);
    }
}

fn format_optional_count(value: Option<u64>) -> String {
    value
        .map(|v| v.to_formatted_string(&Locale::en))
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_optional_ratio(value: Option<u64>, total: u64) -> String {
    match (value, total) {
        (Some(value), total) if total > 0 => format!("{:.2}%", value as f64 / total as f64 * 100.0),
        _ => "N/A".to_string(),
    }
}
