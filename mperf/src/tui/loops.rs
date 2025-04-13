use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    widgets::{Block, Cell, Row, Table, Widget},
};
use sqlite::Connection;

#[derive(Clone)]
pub struct LoopsTab {
    hotspots: Arc<RwLock<Vec<Loop>>>,
    is_running: Arc<RwLock<bool>>,
    connection: Arc<Mutex<Connection>>,
}

#[allow(dead_code)]
struct Loop {
    function_name: String,
    file_name: String,
    line: u32,
    sint_ops: f64,
    sint_ai: f64,
    sfp_ops: f64,
    sfp_ai: f64,
    sdp_ops: f64,
    sdp_ai: f64,
    vint_ops: f64,
    vint_ai: f64,
    vfp_ops: f64,
    vfp_ai: f64,
    vdp_ops: f64,
    vdp_ai: f64,
}

impl Widget for LoopsTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let hotspots = self.hotspots.read();

        if hotspots.is_empty() {
            return;
        }

        let header = [
            Cell::from("Function"),
            Cell::from("Location"),
            Cell::from("Avg. SFLOPs"),
            Cell::from("SFP AI"),
            Cell::from("Avg. SDFLOPs"),
            Cell::from("SDP AI"),
        ]
        .into_iter()
        .collect::<Row>()
        .style(Style::new().bold())
        .height(2);

        let rows = hotspots.iter().map(|loop_| {
            [
                Cell::from(loop_.function_name.as_str()),
                Cell::from(format!("{}:{}", loop_.file_name, loop_.line)),
                Cell::from(format!("{:.2}", loop_.sfp_ops)),
                Cell::from(format!("{:.2}", loop_.sfp_ai)),
                Cell::from(format!("{:.2}", loop_.sdp_ops)),
                Cell::from(format!("{:.2}", loop_.sdp_ai)),
            ]
            .into_iter()
            .collect::<Row>()
        });

        let widths = [
            Constraint::Max(40),
            Constraint::Min(50),
            Constraint::Max(30),
            Constraint::Max(30),
            Constraint::Max(30),
        ];

        let t = Table::new(rows, widths)
            .header(header)
            .block(Block::bordered());

        t.render(area, buf);
    }
}

impl LoopsTab {
    pub fn new(connection: Arc<Mutex<Connection>>) -> Self {
        LoopsTab {
            hotspots: Arc::new(RwLock::new(Vec::new())),
            is_running: Arc::new(RwLock::new(false)),
            connection,
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
            .prepare("SELECT * FROM roofline;")
            .unwrap()
            .into_iter()
            .map(|row| -> Loop {
                let row = row.unwrap();
                Loop {
                    function_name: row.read::<&str, _>("function_name").to_string(),
                    file_name: row.read::<&str, _>("file_name").to_string(),
                    line: row.read::<i64, _>("line") as u32,

                    sint_ops: row.read::<f64, _>("scalar_int_ops"),
                    sint_ai: row.read::<f64, _>("scalar_int_ai"),

                    sfp_ops: row.read::<f64, _>("scalar_float_ops"),
                    sfp_ai: row.read::<f64, _>("scalar_float_ai"),

                    sdp_ops: row.read::<f64, _>("scalar_double_ops"),
                    sdp_ai: row.read::<f64, _>("scalar_double_ai"),

                    vint_ops: row.read::<f64, _>("vector_int_ops"),
                    vint_ai: row.read::<f64, _>("vector_int_ai"),

                    vfp_ops: row.read::<f64, _>("vector_float_ops"),
                    vfp_ai: row.read::<f64, _>("vector_float_ai"),

                    vdp_ops: row.read::<f64, _>("vector_double_ops"),
                    vdp_ai: row.read::<f64, _>("vector_double_ai"),
                }
            })
            .collect();

        let mut hotspots = self.hotspots.write();
        *hotspots = rows;
    }
}
