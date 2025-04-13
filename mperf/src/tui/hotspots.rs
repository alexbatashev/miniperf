use std::sync::Arc;

use num_format::{Locale, ToFormattedString};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::Constraint,
    text::Text,
    widgets::{Block, Cell, Row, Table, Widget},
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
    cache_miss_rate: Option<f64>,
}

macro_rules! oflt {
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
            Cell::from("Total %"),
            Cell::from("Cycles"),
            Cell::from("Instructions"),
            Cell::from("IPC"),
            Cell::from("Branch mispred, %"),
            Cell::from("Cache miss, %"),
        ]
        .into_iter()
        .collect::<Row>();

        let (rows, widths) = get_rows(&hotspots);

        let t = Table::new(rows, widths)
            .header(header)
            .block(Block::bordered());

        t.render(area, buf);
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
                    cache_miss_rate: row.try_read::<f64, _>("cache_miss_rate").ok(),
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
                Cell::new(Text::from(format!("{:.2}%", h.total * 100.0))),
                Cell::new(Text::from(h.cycles.to_formatted_string(&Locale::en))),
                Cell::new(Text::from(h.instructions.to_formatted_string(&Locale::en))),
                Cell::new(Text::from(format!("{:.2}", h.ipc))),
                Cell::new(oflt!(h.branch_miss_rate)),
                Cell::new(oflt!(h.cache_miss_rate)),
            ]
            .into_iter()
            .collect::<Row>()
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Max(30),
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
        Constraint::Max(20),
    ]
    .into_iter()
    .collect();

    (rows, widths)
}
