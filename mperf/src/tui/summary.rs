use std::sync::Arc;

use mperf_data::RecordInfo;
use num_format::Locale;
use num_format::ToFormattedString;
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    widgets::{Block, Row, Table, Widget},
};
use sqlite::Connection;

#[derive(Clone)]
pub struct SummaryTab {
    record_info: RecordInfo,
    connection: Arc<Mutex<Connection>>,
    stat: Arc<RwLock<Stat>>,
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
        }
    }

    pub fn run(&self) {
        {
            let stat = self.stat.read();
            if stat.cycles != 0 {
                return;
            }
        }
        let this = self.clone();
        tokio::spawn(this.fetch_data());
    }

    async fn fetch_data(self) {
        let conn = self.connection.lock();
        let mut row = conn
            .prepare(
                "SELECT
            SUM(pmu_cycles) as pmu_cycles,
            SUM(pmu_instructions) as pmu_instructions,
            SUM(pmu_llc_references) as pmu_llc_references,
            SUM(pmu_llc_misses) as pmu_llc_misses,
            SUM(pmu_branch_instructions) as pmu_branch_instructions,
            SUM(pmu_branch_misses) as pmu_branch_misses,
            SUM(pmu_stalled_cycles_frontend) as pmu_stalled_cycles_frontend,
            SUM(pmu_stalled_cycles_backend) as pmu_stalled_cycles_backend

            FROM pmu_counters;
        ",
            )
            .unwrap()
            .into_iter();

        let row = row.next().unwrap().unwrap();

        println!("{:?}", &row);

        let mut stat = self.stat.write();
        *stat = Stat {
            cycles: row.read::<i64, _>("pmu_cycles") as u64,
            instructions: row.read::<i64, _>("pmu_instructions") as u64,
            branch_instructions: row.read::<i64, _>("pmu_branch_instructions") as u64,
            branch_misses: row.read::<i64, _>("pmu_branch_misses") as u64,
            cache_references: row.read::<i64, _>("pmu_llc_references") as u64,
            cache_misses: row.read::<i64, _>("pmu_llc_misses") as u64,
            stalled_cycles_frontend: row.read::<i64, _>("pmu_stalled_cycles_frontend") as u64,
            stalled_cycles_backend: row.read::<i64, _>("pmu_stalled_cycles_backend") as u64,
        };
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

            if stat.cycles == 0 {
                let counter = 0;
                let pb = ratatui::widgets::Gauge::default()
                    .block(Block::bordered().title("Loading data..."))
                    .gauge_style(Style::new().white().on_black().italic())
                    .percent(counter);
                pb.render(stat_table_area, buf);
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
