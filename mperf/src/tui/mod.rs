use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use crossterm::event::{EventStream, KeyCode, KeyEventKind};
use memmap2::{Advice, Mmap};
use mperf_data::{Event, EventType, RecordInfo, Scenario};
use num_format::{Locale, ToFormattedString};
use parking_lot::RwLock;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, Gauge, Row, Table, Tabs, Widget},
    DefaultTerminal, Frame,
};
use tokio::fs::{self, File};
use tokio_stream::StreamExt;

pub async fn tui_main(res_dir: &Path) -> Result<()> {
    let terminal = ratatui::init();
    let app_result = App::new(res_dir).run(terminal).await;
    ratatui::restore();
    app_result
}

#[derive(Default)]
struct App {
    should_quit: bool,
    tabs: TabsWidget,
    res_dir: PathBuf,
}

impl App {
    const FRAMES_PER_SECOND: f32 = 120.0;

    pub fn new(res_dir: &Path) -> Self {
        App {
            res_dir: res_dir.to_owned(),
            ..Default::default()
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.tabs.run(&self.res_dir);

        let period = Duration::from_secs_f32(1.0 / Self::FRAMES_PER_SECOND);
        let mut interval = tokio::time::interval(period);
        let mut events = EventStream::new();

        while !self.should_quit {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(|frame| self.draw(frame))?; },
                Some(Ok(event)) = events.next() => self.handle_event(&event),
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let vertical = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]);
        let [title_area, body_area] = vertical.areas(frame.area());
        let title = Line::from("mperf results").centered().bold();
        frame.render_widget(title, title_area);
        frame.render_widget(&self.tabs, body_area);
    }

    fn handle_event(&mut self, event: &crossterm::event::Event) {
        if let crossterm::event::Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                    // KeyCode::Char('j') | KeyCode::Down => self.pull_requests.scroll_down(),
                    // KeyCode::Char('k') | KeyCode::Up => self.pull_requests.scroll_up(),
                    _ => {}
                }
            }
        }
    }
}

#[derive(Default, Clone)]
struct TabsWidget {
    cur_tab: usize,
    tabs: Arc<RwLock<Vec<Tab>>>,
}

impl Widget for &TabsWidget {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let read_tabs = self.tabs.read();
        if read_tabs.len() == 0 {
            return;
        }
        let tabs = Tabs::new(read_tabs.iter().map(|t| t.name()))
            .style(Style::default().white())
            .highlight_style(Style::default().yellow())
            .select(self.cur_tab);

        tabs.render(area, buf);

        let vertical = Layout::vertical_margin(Layout::vertical([Constraint::Fill(1)]), 1);
        let [area] = vertical.areas(area);
        read_tabs[self.cur_tab].run();
        read_tabs[self.cur_tab].render(area, buf);
    }
}

#[derive(Clone)]
enum Tab {
    Summary(SummaryTab),
}

#[derive(Clone)]
struct SummaryTab {
    res_dir: PathBuf,
    record_info: RecordInfo,
    stat: Arc<RwLock<Stat>>,
    counter: Arc<RwLock<u16>>,
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

impl Tab {
    fn name(&self) -> &'static str {
        match self {
            Tab::Summary(_) => "Summary",
        }
    }

    fn run(&self) {
        match self {
            Tab::Summary(summary) => summary.run(),
        }
    }
}

impl TabsWidget {
    fn run(&self, res_dir: &Path) {
        {
            let read_tabs = self.tabs.read();

            for tab in read_tabs.iter() {
                tab.run();
            }

            if !read_tabs.is_empty() {
                return;
            }
        }
        let this = self.clone();
        tokio::spawn(this.fetch_data(res_dir.to_owned()));
    }

    async fn fetch_data(self, res_dir: PathBuf) {
        let data = fs::read_to_string(res_dir.join("info.json"))
            .await
            .expect("failed to read info.json");
        let info: RecordInfo = serde_json::from_str(&data).expect("failed to parse info.json");

        let mut write_tabs = self.tabs.write();

        match info.scenario {
            Scenario::Snapshot => {
                write_tabs.push(Tab::Summary(SummaryTab::new(res_dir.clone(), info.clone())));
            }
            _ => unimplemented!(),
        }
    }
}

impl SummaryTab {
    fn new(res_dir: PathBuf, record_info: RecordInfo) -> Self {
        SummaryTab {
            res_dir,
            record_info,
            stat: Arc::new(RwLock::new(Stat::default())),
            counter: Arc::new(RwLock::new(80)),
        }
    }

    fn run(&self) {
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
        let file = File::open(self.res_dir.join("events.bin"))
            .await
            .expect("failed to open events.bin");

        let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
        map.advise(Advice::Sequential)
            .expect("Failed to advice sequential reads");

        let data_stream = unsafe {
            std::slice::from_raw_parts(map.as_ptr(), map.len())
        };

        let mut cursor = std::io::Cursor::new(data_stream);

        let mut stat = Stat::default();

        while (cursor.position() as usize) < map.len() {
            // FIXME: should we just skip?
            let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

            {
                let mut cntr = self.counter.write();
                *cntr = (100 * cursor.position() / data_stream.len() as u64) as u16;
            }

            if evt.time_running == 0 {
                continue;
            }

            let value =
                (evt.value as f64 * (evt.time_enabled as f64 / evt.time_running as f64)) as u64;

            match evt.ty {
                EventType::PmuCycles => stat.cycles += value,
                EventType::PmuInstructions => stat.instructions += value,
                EventType::PmuLlcReferences => stat.cache_references += value,
                EventType::PmuLlcMisses => stat.cache_misses += value,
                EventType::PmuBranchMisses => stat.branch_misses += value,
                EventType::PmuBranchInstructions => stat.branch_instructions += value,
                EventType::PmuStalledCyclesBackend => stat.stalled_cycles_backend += value,
                EventType::PmuStalledCyclesFrontend => stat.stalled_cycles_frontend += value,
                _ => {}
            };
        }

        // let events = unsafe {
        //     std::slice::from_raw_parts(
        //         map.as_ptr() as *const Event,
        //         map.len() / std::mem::size_of::<Event>(),
        //     )
        // };


        // for (i, evt) in events.iter().enumerate() {
        //
        // }

        {
            let mut cntr = self.counter.write();
            *cntr = 50;
        }

        let mut write_stat = self.stat.write();
        *write_stat = stat;

        {
            let mut cntr = self.counter.write();
            *cntr = 50;
        }
    }
}

impl Widget for &Tab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        match self {
            Tab::Summary(tab) => tab.clone().render(area, buf),
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

            if stat.cycles == 0 {
                let counter = self.counter.read();
                let pb = Gauge::default()
                    .block(Block::bordered().title("Loading data..."))
                    .gauge_style(Style::new().white().on_black().italic())
                    .percent(*counter);
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
