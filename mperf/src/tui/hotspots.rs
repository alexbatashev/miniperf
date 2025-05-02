use std::sync::Arc;

use num_format::{Locale, ToFormattedString};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Text,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget},
};
use sqlite::Connection;

#[derive(Clone)]
pub struct HotspotsTab {
    hotspots: Arc<RwLock<Vec<HSRow>>>,
    is_running: Arc<RwLock<bool>>,
    connection: Arc<Mutex<Connection>>,
}

struct HSRow {
    func_name: String,
    total: f64,
    cycles: u64,
    instructions: u64,
    ipc: f64,
    branch_miss_rate: Option<f64>,
    branch_mpki: Option<f64>,
    cache_miss_rate: Option<f64>,
    cache_mpki: Option<f64>,
}

macro_rules! oflt {
    ($e:expr) => {
        $e.map(|v| Text::from(format!("{:.2}", v)))
            .unwrap_or(Text::from("N/A"))
    };
}

macro_rules! ofltp {
    ($e:expr) => {
        $e.map(|v| Text::from(format!("{:.2}%", v * 100.0)))
            .unwrap_or(Text::from("N/A"))
    };
}

impl Widget for HotspotsTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let hotspots = self.hotspots.read();

        let header = [
            Cell::from("Function"),
            Cell::from(Text::from("Total %").alignment(Alignment::Right)),
            Cell::from(Text::from("Cycles").alignment(Alignment::Right)),
            Cell::from(Text::from("Instructions").alignment(Alignment::Right)),
            Cell::from(Text::from("IPC").alignment(Alignment::Right)),
            Cell::from(Text::from("Branch MPKI").alignment(Alignment::Right)),
            Cell::from(Text::from("Branch mispred, %").alignment(Alignment::Right)),
            Cell::from(Text::from("Cache MPKI").alignment(Alignment::Right)),
            Cell::from(Text::from("Cache miss, %").alignment(Alignment::Right)),
        ]
        .into_iter()
        .collect::<Row>()
        .height(2)
        .style(Style::new().bold());

        let (rows, widths) = get_rows(&hotspots);

        let vertical = Layout::vertical([Constraint::Fill(1)]).vertical_margin(0);
        let horizontal = Layout::horizontal([Constraint::Fill(1)]).horizontal_margin(0);

        let [table_area] = vertical.areas(area);
        let [table_area] = horizontal.areas(table_area);

        let t = Table::new(rows, widths)
            .header(header)
            .block(Block::new().borders(Borders::TOP | Borders::BOTTOM));

        let header_separator_y = table_area.y + 2;
        if header_separator_y < table_area.bottom() - 1 {
            let line_area = Rect::new(table_area.x, header_separator_y, table_area.width, 1);
            // Draw a horizontal line using a Paragraph widget
            let line = "─".repeat(table_area.width as usize);
            let line_widget = Paragraph::new(line.as_str()).style(Style::default());
            line_widget.render(line_area, buf);
        }

        t.render(table_area, buf);
    }
}

impl HotspotsTab {
    pub fn new(connection: Arc<Mutex<Connection>>) -> Self {
        HotspotsTab {
            hotspots: Arc::new(RwLock::new(Vec::new())),
            connection,
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    pub fn run(&self) {
        {
            let hotspots = self.hotspots.read();
            if !hotspots.is_empty() || *self.is_running.read() {
                return;
            }
        }
        *self.is_running.write() = true;
        let this = self.clone();
        tokio::spawn(this.fetch_data());
    }

    async fn fetch_data(self) {
        let conn = self.connection.lock();
        let rows = conn
            .prepare("SELECT * FROM hotspots ORDER BY total DESC LIMIT 50;")
            .unwrap()
            .into_iter()
            .map(|row| -> HSRow {
                let row = row.unwrap();
                HSRow {
                    func_name: row.read::<&str, _>("func_name").to_string(),
                    total: row.read::<f64, _>("total"),
                    cycles: row.read::<i64, _>("cycles") as u64,
                    instructions: row.read::<i64, _>("instructions") as u64,
                    ipc: row.read::<f64, _>("ipc") as f64,
                    branch_miss_rate: row.try_read::<f64, _>("branch_miss_rate").ok(),
                    branch_mpki: row.try_read::<f64, _>("branch_mpki").ok(),
                    cache_miss_rate: row.try_read::<f64, _>("cache_miss_rate").ok(),
                    cache_mpki: row.try_read::<f64, _>("cache_mpki").ok(),
                }
            })
            .collect();

        let mut hotspots = self.hotspots.write();
        *hotspots = rows;
    }
}

fn get_rows(hotspots: &[HSRow]) -> (Vec<Row<'_>>, Vec<Constraint>) {
    let rows = hotspots
        .iter()
        .map(|h| {
            [
                Cell::new(h.func_name.clone()),
                Cell::new(Text::from(ofltp!(Some(h.total))).alignment(Alignment::Right)),
                Cell::new(
                    Text::from(h.cycles.to_formatted_string(&Locale::en))
                        .alignment(Alignment::Right),
                ),
                Cell::new(
                    Text::from(h.instructions.to_formatted_string(&Locale::en))
                        .alignment(Alignment::Right),
                ),
                Cell::new(Text::from(oflt!(Some(h.ipc))).alignment(Alignment::Right)),
                Cell::new(Text::from(oflt!(h.branch_mpki)).alignment(Alignment::Right)),
                Cell::new(Text::from(ofltp!(h.branch_miss_rate)).alignment(Alignment::Right)),
                Cell::new(Text::from(oflt!(h.cache_mpki)).alignment(Alignment::Right)),
                Cell::new(Text::from(ofltp!(h.cache_miss_rate)).alignment(Alignment::Right)),
            ]
            .into_iter()
            .collect::<Row>()
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Max(30),
        Constraint::Max(10), // Total %
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(10), // IPC
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
    ]
    .into_iter()
    .collect();

    (rows, widths)
}
