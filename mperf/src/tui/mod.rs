use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use crossterm::event::{EventStream, KeyCode, KeyEventKind};
use hotspots::HotspotsTab;
use loops::LoopsTab;
use mperf_data::{RecordInfo, Scenario};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Constraint, Layout},
    style::{palette, Style, Stylize},
    text::Line,
    widgets::{Tabs, Widget},
    DefaultTerminal, Frame,
};
use summary::SummaryTab;
use tokio::fs::{self};
use tokio_stream::StreamExt;

mod hotspots;
mod loops;
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
    }

    fn handle_event(&mut self, event: &crossterm::event::Event) {
        if let crossterm::event::Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                    KeyCode::Tab => self.tabs.next_tab(),
                    KeyCode::BackTab => self.tabs.previous_tab(),
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
    Hotspots(HotspotsTab),
    Loops(LoopsTab),
}

impl Tab {
    fn name(&self) -> &'static str {
        match self {
            Tab::Summary(_) => " Summary ",
            Tab::Hotspots(_) => " Hotspots ",
            Tab::Loops(_) => " Loops ",
        }
    }

    fn run(&self) {
        match self {
            Tab::Summary(summary) => summary.run(),
            Tab::Hotspots(hotspots) => hotspots.run(),
            Tab::Loops(loops) => loops.run(),
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

        match info.scenario {
            Scenario::Snapshot => {
                write_tabs.push(Tab::Summary(SummaryTab::new(
                    info.clone(),
                    connection.clone(),
                )));
                write_tabs.push(Tab::Hotspots(HotspotsTab::new(connection.clone())));
            }
            Scenario::Roofline => {
                write_tabs.push(Tab::Summary(SummaryTab::new(
                    info.clone(),
                    connection.clone(),
                )));
                write_tabs.push(Tab::Loops(LoopsTab::new(connection.clone())));
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
}

impl Widget for &Tab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        match self {
            Tab::Summary(tab) => tab.clone().render(area, buf),
            Tab::Hotspots(tab) => tab.clone().render(area, buf),
            Tab::Loops(tab) => tab.clone().render(area, buf),
        }
    }
}
