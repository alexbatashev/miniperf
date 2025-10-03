use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use config::scenario_ui;
use crossterm::event::{EventStream, KeyCode, KeyEventKind};
use flamegraph::FlamegraphTab;
use loops::LoopsTab;
use metrics_table::MetricsTableTab;
use mperf_data::{RecordInfo, Scenario};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{palette, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Clear, Row, Table, Tabs, Widget},
    DefaultTerminal, Frame,
};
use summary::SummaryTab;
use tokio::fs::{self};
use tokio_stream::StreamExt;

mod config;
mod flamegraph;
mod loops;
mod metrics_table;
mod summary;

pub async fn tui_main(res_dir: &Path) -> Result<()> {
    let terminal = ratatui::init();
    let app_result = App::new(res_dir).run(terminal).await;
    ratatui::restore();
    app_result
}

#[derive(Default)]
struct App {
    should_quit: bool,
    show_help: bool,
    tabs: TabsWidget,
    res_dir: PathBuf,
}

impl App {
    const FRAMES_PER_SECOND: f32 = 30.0;

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

        if self.show_help {
            let block = Block::bordered().title("Help");

            let vertical = Layout::vertical([Constraint::Length(20)]).flex(Flex::Center);
            let horizontal = Layout::horizontal([Constraint::Length(50)]).flex(Flex::Center);

            let [area] = vertical.areas(frame.area());
            let [area] = horizontal.areas(area);

            frame.render_widget(Clear, area);
            frame.render_widget(block, area);

            let header = [Cell::from("Key"), Cell::from("Action")]
                .into_iter()
                .collect::<Row>()
                .style(Style::new().bold());

            let rows = [
                [Cell::from("?"), Cell::from("Show/hide this window")]
                    .into_iter()
                    .collect::<Row>(),
                [Cell::from("q"), Cell::from("Quit miniperf")]
                    .into_iter()
                    .collect::<Row>(),
                [Cell::from("<tab>"), Cell::from("Switch tabs")]
                    .into_iter()
                    .collect::<Row>(),
            ];

            let vertical = Layout::vertical_margin(Layout::vertical([Constraint::Fill(1)]), 2);
            let horizontal =
                Layout::horizontal_margin(Layout::horizontal([Constraint::Fill(1)]), 2);

            let [table_area] = vertical.areas(area);
            let [table_area] = horizontal.areas(table_area);

            let widths = [Constraint::Length(8), Constraint::Fill(1)];
            let t = Table::new(rows, widths).header(header);
            frame.render_widget(t, table_area);
        }
    }

    fn handle_event(&mut self, event: &crossterm::event::Event) {
        if let crossterm::event::Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Tab => {
                        if !self.show_help {
                            self.tabs.next_tab()
                        }
                    }
                    KeyCode::BackTab => {
                        if !self.show_help {
                            self.tabs.previous_tab()
                        }
                    }
                    KeyCode::Char('?') => self.show_help = !self.show_help,
                    _ => {
                        if !self.show_help {
                            self.tabs.handle_event(key.code);
                        }
                    }
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

        let highlight_style = (
            ratatui::style::Color::default(),
            palette::tailwind::EMERALD.c700,
        );

        let tabs = Tabs::new(read_tabs.iter().map(|t| t.name()))
            .style(Style::default().white())
            .highlight_style(highlight_style)
            .divider(ratatui::symbols::DOT)
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
    MetricsTable(MetricsTableTab),
    Loops(LoopsTab),
    Flamegraph(FlamegraphTab),
}

impl Tab {
    fn name(&self) -> String {
        match self {
            Tab::Summary(_) => " Summary ".to_string(),
            Tab::MetricsTable(tab) => tab.title().to_string(),
            Tab::Loops(_) => " Loops ".to_string(),
            Tab::Flamegraph(_) => " Flamegraph ".to_string(),
        }
    }

    fn run(&self) {
        match self {
            Tab::Summary(summary) => summary.run(),
            Tab::MetricsTable(table) => table.run(),
            Tab::Loops(loops) => loops.run(),
            Tab::Flamegraph(fg) => fg.run(),
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

        let connection = Arc::new(Mutex::new(
            sqlite::open(res_dir.join("perf.db")).expect("Failed to open DB"),
        ));

        let ui = scenario_ui(&info);

        for tab in ui.tabs.iter() {
            match tab {
                pmu_data::TabSpec::Summary => write_tabs.push(Tab::Summary(SummaryTab::new(
                    info.clone(),
                    connection.clone(),
                ))),
                pmu_data::TabSpec::Flamegraph => {
                    write_tabs.push(Tab::Flamegraph(FlamegraphTab::new(res_dir.clone())))
                }
                pmu_data::TabSpec::Loops => {
                    if matches!(info.scenario, Scenario::Roofline) {
                        write_tabs.push(Tab::Loops(LoopsTab::new(connection.clone())));
                    }
                }
                pmu_data::TabSpec::MetricsTable(spec) => write_tabs.push(Tab::MetricsTable(
                    MetricsTableTab::new(spec.clone(), connection.clone()),
                )),
            }
        }
    }

    fn next_tab(&mut self) {
        self.cur_tab += 1;
        if self.cur_tab >= self.tabs.read().len() {
            self.cur_tab = 0;
        }
    }

    fn previous_tab(&mut self) {
        if self.cur_tab == 0 {
            self.cur_tab = self.tabs.read().len() - 1;
        } else {
            self.cur_tab -= 1;
        }
    }

    fn handle_event(&mut self, code: KeyCode) {
        match &mut self.tabs.write()[self.cur_tab] {
            Tab::MetricsTable(tab) => tab.handle_event(code),
            Tab::Flamegraph(tab) => tab.handle_event(code),
            _ => {}
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
            Tab::MetricsTable(tab) => tab.clone().render(area, buf),
            Tab::Loops(tab) => tab.clone().render(area, buf),
            Tab::Flamegraph(tab) => tab.clone().render(area, buf),
        }
    }
}
