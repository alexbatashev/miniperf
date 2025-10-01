use std::sync::Arc;

use crossterm::event::KeyCode;
use num_format::{Locale, ToFormattedString};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Text,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Widget},
};
use sqlite::Connection;

#[derive(Clone)]
pub struct HotspotsTab {
    hotspots: Arc<RwLock<Vec<HSRow>>>,
    is_running: Arc<RwLock<bool>>,
    connection: Arc<Mutex<Connection>>,
    state: Arc<Mutex<HotspotsState>>,
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

#[derive(Default)]
struct HotspotsState {
    selected: Option<usize>,
    offset: usize,
    column_offset: usize,
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

        let vertical = Layout::vertical([Constraint::Fill(1)]).vertical_margin(0);
        let horizontal = Layout::horizontal([Constraint::Fill(1)]).horizontal_margin(0);

        let [table_area] = vertical.areas(area);
        let [table_area] = horizontal.areas(table_area);

        let mut state = self.state.lock();
        let total_rows = hotspots.len();

        if total_rows == 0 {
            state.selected = None;
            state.offset = 0;
        } else {
            if state.selected.is_none() {
                state.selected = Some(0);
            }
            if let Some(selected) = state.selected {
                if selected >= total_rows {
                    state.selected = Some(total_rows - 1);
                }
            }
        }

        let metric_columns = METRIC_COLUMN_COUNT;
        let max_metric_offset = metric_columns.saturating_sub(1);
        if state.column_offset > max_metric_offset {
            state.column_offset = max_metric_offset;
        }
        let column_offset = state.column_offset;

        let mut header_cells = header_cells();
        let sticky_header = header_cells.remove(0);
        let header = Row::new(
            std::iter::once(sticky_header).chain(header_cells.into_iter().skip(column_offset)),
        )
        .height(2)
        .style(Style::new().bold());

        let rows = hotspots
            .iter()
            .map(|row| {
                let mut cells = row_cells(row);
                let sticky_cell = cells.remove(0);
                Row::new(std::iter::once(sticky_cell).chain(cells.into_iter().skip(column_offset)))
            })
            .collect::<Vec<_>>();

        let mut widths = column_constraints();
        let sticky_width = widths.remove(0);
        let widths = std::iter::once(sticky_width)
            .chain(widths.into_iter().skip(column_offset))
            .collect::<Vec<_>>();

        let table_height = table_area.height as usize;
        let header_height = 2usize;
        let border_and_separator_height = 3usize; // top border + separator line + bottom border
        let visible_rows = table_height
            .saturating_sub(header_height + border_and_separator_height)
            .max(1);

        if let Some(selected) = state.selected {
            if selected < state.offset {
                state.offset = selected;
            } else if selected >= state.offset + visible_rows {
                state.offset = selected + 1 - visible_rows;
            }
        } else {
            state.offset = 0;
        }

        let mut table_state = TableState::default()
            .with_selected(state.selected)
            .with_offset(state.offset);

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("▶ ")
            .block(Block::new().borders(Borders::TOP | Borders::BOTTOM));

        let header_separator_y = table_area.y + 2;
        if header_separator_y < table_area.bottom() - 1 {
            let line_area = Rect::new(table_area.x, header_separator_y, table_area.width, 1);
            let line = "─".repeat(table_area.width as usize);
            let line_widget = Paragraph::new(line.as_str()).style(Style::default());
            line_widget.render(line_area, buf);
        }

        ratatui::widgets::StatefulWidget::render(table, table_area, buf, &mut table_state);

        state.selected = table_state.selected();
        state.offset = table_state.offset();
    }
}

impl HotspotsTab {
    pub fn new(connection: Arc<Mutex<Connection>>) -> Self {
        HotspotsTab {
            hotspots: Arc::new(RwLock::new(Vec::new())),
            connection,
            is_running: Arc::new(RwLock::new(false)),
            state: Arc::new(Mutex::new(HotspotsState::default())),
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

        let mut state = self.state.lock();
        state.selected = if hotspots.is_empty() { None } else { Some(0) };
        state.offset = 0;
        state.column_offset = 0;
    }
}

impl HotspotsTab {
    pub fn handle_event(&mut self, code: KeyCode) {
        let hotspots_len = self.hotspots.read().len();
        if hotspots_len == 0 {
            return;
        }

        let mut state = self.state.lock();
        let max_metric_offset = METRIC_COLUMN_COUNT.saturating_sub(1);
        match code {
            KeyCode::Down => {
                let current = state.selected.unwrap_or(0);
                let next = (current + 1).min(hotspots_len - 1);
                state.selected = Some(next);
            }
            KeyCode::Up => {
                let current = state.selected.unwrap_or(0);
                let next = current.saturating_sub(1);
                state.selected = Some(next);
            }
            KeyCode::PageDown => {
                let step = 5;
                let current = state.selected.unwrap_or(0);
                let next = (current + step).min(hotspots_len - 1);
                state.selected = Some(next);
                let new_offset = state.offset.saturating_add(step);
                state.offset = new_offset.min(hotspots_len.saturating_sub(1));
            }
            KeyCode::PageUp => {
                let step = 5;
                let current = state.selected.unwrap_or(0);
                let next = current.saturating_sub(step);
                state.selected = Some(next);
                state.offset = state.offset.saturating_sub(step);
            }
            KeyCode::Home => {
                state.selected = Some(0);
                state.offset = 0;
            }
            KeyCode::End => {
                state.selected = Some(hotspots_len - 1);
            }
            KeyCode::Right => {
                if state.column_offset < max_metric_offset {
                    state.column_offset += 1;
                }
            }
            KeyCode::Left => {
                state.column_offset = state.column_offset.saturating_sub(1);
            }
            _ => {}
        }
    }
}

const COLUMN_COUNT: usize = 9;
const STICKY_COLUMN_COUNT: usize = 1;
const METRIC_COLUMN_COUNT: usize = COLUMN_COUNT - STICKY_COLUMN_COUNT;

fn header_cells() -> Vec<Cell<'static>> {
    vec![
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
}

fn row_cells(h: &HSRow) -> Vec<Cell<'static>> {
    vec![
        Cell::from(h.func_name.clone()),
        Cell::from(ofltp!(Some(h.total)).alignment(Alignment::Right)),
        Cell::from(
            Text::from(h.cycles.to_formatted_string(&Locale::en)).alignment(Alignment::Right),
        ),
        Cell::from(
            Text::from(h.instructions.to_formatted_string(&Locale::en)).alignment(Alignment::Right),
        ),
        Cell::from(oflt!(Some(h.ipc)).alignment(Alignment::Right)),
        Cell::from(oflt!(h.branch_mpki).alignment(Alignment::Right)),
        Cell::from(ofltp!(h.branch_miss_rate).alignment(Alignment::Right)),
        Cell::from(oflt!(h.cache_mpki).alignment(Alignment::Right)),
        Cell::from(ofltp!(h.cache_miss_rate).alignment(Alignment::Right)),
    ]
}

fn column_constraints() -> Vec<Constraint> {
    vec![
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
}
