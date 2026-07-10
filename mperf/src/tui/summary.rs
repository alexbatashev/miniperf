use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
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
    load_complete: Arc<AtomicBool>,
    load_error: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Stat {
    cycles: u64,
    instructions: u64,
    branch_instructions: u64,
    branch_misses: u64,
    cache_references: u64,
    cache_misses: u64,
    stalled_cycles_frontend: u64,
    stalled_cycles_backend: u64,
}

impl SummaryTab {
    pub fn new(record_info: RecordInfo, connection: Arc<Mutex<Connection>>) -> Self {
        SummaryTab {
            record_info,
            connection,
            stat: Arc::new(RwLock::new(Stat::default())),
            load_started: Arc::new(AtomicBool::new(false)),
            load_complete: Arc::new(AtomicBool::new(false)),
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
            use sqlite::State;

            let mut statement = conn
            .prepare(
                "SELECT
            SUM(pmu_cycles) as pmu_cycles,
            SUM(pmu_instructions) as pmu_instructions,
            CAST(SUM(pmu_llc_references * 1.0 / confidence) AS INTEGER) AS pmu_llc_references,
            CAST(SUM(pmu_llc_misses * 1.0 / confidence) AS INTEGER) AS pmu_llc_misses,
            CAST(SUM(pmu_branch_instructions * 1.0 / confidence) AS INTEGER) AS pmu_branch_instructions,
            CAST(SUM(pmu_branch_misses * 1.0 / confidence) AS INTEGER) AS pmu_branch_misses,
            CAST(SUM(pmu_stalled_cycles_frontend * 1.0 / confidence) AS INTEGER) AS pmu_stalled_cycles_frontend,
            CAST(SUM(pmu_stalled_cycles_backend * 1.0 / confidence) AS INTEGER) AS pmu_stalled_cycles_backend

            FROM pmu_counters;
        ",
            )
            .map_err(|error| error.to_string())?;

            if statement.next().map_err(|error| error.to_string())? != State::Row {
                return Err("summary query returned no rows".to_string());
            }

            let read = |name| {
                statement
                    .read::<Option<i64>, _>(name)
                    .map(|value| value.unwrap_or_default() as u64)
                    .map_err(|error| error.to_string())
            };
            Ok(Stat {
                cycles: read("pmu_cycles")?,
                instructions: read("pmu_instructions")?,
                branch_instructions: read("pmu_branch_instructions")?,
                branch_misses: read("pmu_branch_misses")?,
                cache_references: read("pmu_llc_references")?,
                cache_misses: read("pmu_llc_misses")?,
                stalled_cycles_frontend: read("pmu_stalled_cycles_frontend")?,
                stalled_cycles_backend: read("pmu_stalled_cycles_backend")?,
            })
        })();
        drop(conn);

        match result {
            Ok(stat) => {
                *self.stat.write() = stat;
                self.load_complete.store(true, Ordering::Release);
            }
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
        if let Some(error) = self.load_error.read().clone() {
            Paragraph::new(error)
                .block(Block::bordered().title("Summary error"))
                .wrap(Wrap { trim: true })
                .render(area, buf);
            return;
        }

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

            if !self.load_complete.load(Ordering::Acquire) {
                let counter = 0;
                let pb = ratatui::widgets::Gauge::default()
                    .block(Block::bordered().title("Loading data..."))
                    .gauge_style(Style::new().white().on_black().italic())
                    .percent(counter);
                pb.render(stat_table_area, buf);
            } else if stat.cycles == 0 {
                Paragraph::new("No counter samples were found in this recording.")
                    .wrap(Wrap { trim: true })
                    .render(stat_table_area, buf);
            } else {
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
                    Row::new([
                        "IPC".to_string(),
                        format!("{:.2}", stat.instructions as f64 / stat.cycles as f64),
                        "".to_string(),
                    ]),
                    Row::new([
                        "Branch instructions".to_string(),
                        stat.branch_instructions.to_formatted_string(&Locale::en),
                        format!(
                            "{:.2} per cycle",
                            stat.branch_instructions as f64 / stat.cycles as f64
                        ),
                    ]),
                    Row::new([
                        "Branch misses".to_string(),
                        stat.branch_misses.to_formatted_string(&Locale::en),
                        format!(
                            "{:.2}%",
                            stat.branch_misses as f64 / stat.branch_instructions as f64 * 100_f64
                        ),
                    ]),
                    Row::new([
                        "Branch MPKI".to_string(),
                        format!(
                            "{:.2}",
                            stat.branch_misses as f64 / stat.instructions as f64 * 1000.0
                        ),
                        "".to_string(),
                    ]),
                    Row::new([
                        "Last level cache references".to_string(),
                        stat.cache_references.to_formatted_string(&Locale::en),
                        "".to_string(),
                    ]),
                    Row::new([
                        "Last level cache misses".to_string(),
                        stat.cache_misses.to_formatted_string(&Locale::en),
                        format!(
                            "{:.2}%",
                            stat.cache_misses as f64
                                / (stat.cache_misses + stat.cache_references) as f64
                                * 100_f64
                        ),
                    ]),
                    Row::new([
                        "Cache MPKI".to_string(),
                        format!(
                            "{:.2}",
                            stat.cache_misses as f64 / stat.instructions as f64 * 1000.0
                        ),
                        "".to_string(),
                    ]),
                    Row::new([
                        "Stalled cycles backend".to_string(),
                        stat.stalled_cycles_backend.to_formatted_string(&Locale::en),
                        format!(
                            "{:.2}%",
                            stat.stalled_cycles_backend as f64 / stat.cycles as f64 * 100.0
                        ),
                    ]),
                    Row::new([
                        "Stalled cycles frontend".to_string(),
                        stat.stalled_cycles_frontend
                            .to_formatted_string(&Locale::en),
                        format!(
                            "{:.2}%",
                            stat.stalled_cycles_frontend as f64 / stat.cycles as f64 * 100.0
                        ),
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
