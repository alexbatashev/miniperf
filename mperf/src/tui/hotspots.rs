use std::{collections::HashMap, sync::Arc};

use crossterm::event::KeyCode;
use num_format::{Locale, ToFormattedString};
use parking_lot::{Mutex, RwLock};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Widget},
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

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum HotspotsFocus {
    #[default]
    List,
    Assembly,
}

#[derive(Default)]
struct HotspotsState {
    selected: Option<usize>,
    offset: usize,
    column_offset: usize,
    focus: HotspotsFocus,
    assembly_loading: bool,
    assembly_error: Option<String>,
    assembly: Option<AssemblyViewState>,
    assembly_request_id: u64,
}

#[derive(Clone)]
struct AssemblyRow {
    address: u64,
    instruction: String,
    samples: u64,
    share: f64,
    cycles: u64,
    instructions: u64,
    branch_misses: u64,
    branch_instructions: u64,
    llc_misses: u64,
    llc_references: u64,
}

#[derive(Clone)]
struct AssemblyViewState {
    func_name: String,
    module_path: String,
    symbol: String,
    rows: Vec<AssemblyRow>,
    selected: Option<usize>,
    offset: usize,
    max_samples: u64,
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

        let assembly_needed =
            state.assembly_loading || state.assembly.is_some() || state.assembly_error.is_some();

        let mut overlay_area = None;
        if assembly_needed {
            overlay_area = Some(area);

            if let Some(assembly) = state.assembly.as_mut() {
                let available_height = area.height.saturating_sub(6) as usize;
                let visible_rows = available_height.max(1);
                if let Some(selected) = assembly.selected {
                    if selected < assembly.offset {
                        assembly.offset = selected;
                    } else if selected >= assembly.offset + visible_rows {
                        assembly.offset = selected + 1 - visible_rows;
                    }
                } else {
                    assembly.offset = 0;
                }
            }
        }

        let assembly_view = state.assembly.clone();
        let assembly_loading = state.assembly_loading;
        let assembly_error = state.assembly_error.clone();
        drop(state);

        if let Some(overlay) = overlay_area {
            self.render_assembly_overlay(
                buf,
                overlay,
                assembly_loading,
                assembly_error,
                assembly_view,
            );
        }
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

    fn request_assembly(&self, func_name: String) {
        let request_id = {
            let mut state = self.state.lock();
            state.assembly_request_id = state.assembly_request_id.wrapping_add(1);
            state.focus = HotspotsFocus::Assembly;
            state.assembly_loading = true;
            state.assembly_error = None;
            state.assembly = None;
            state.assembly_request_id
        };

        let this = self.clone();
        tokio::spawn(this.fetch_assembly(func_name, request_id));
    }

    async fn fetch_assembly(self, func_name: String, request_id: u64) {
        let result: Result<AssemblyViewState, String> = (|| {
            use sqlite::State;

            let conn = self.connection.lock();

            let mut module_stmt = conn
                .prepare(
                    "SELECT proc_map.module_path AS module_path, SUM(pmu_counters.pmu_cycles) AS total_cycles
                     FROM pmu_counters
                     INNER JOIN proc_map ON proc_map.ip = pmu_counters.ip
                     WHERE proc_map.func_name = ?
                     GROUP BY proc_map.module_path
                     ORDER BY total_cycles DESC
                     LIMIT 1;",
                )
                .map_err(|err| err.to_string())?;
            module_stmt
                .bind((1, func_name.as_str()))
                .map_err(|err| err.to_string())?;

            let module_path = match module_stmt.next().map_err(|err| err.to_string())? {
                State::Row => module_stmt
                    .read::<String, _>(0)
                    .map_err(|err| err.to_string())?,
                State::Done => {
                    return Err(
                        "Assembly data is not available for the selected hotspot".to_string()
                    );
                }
            };

            let mut stats_stmt = conn
                .prepare(
                    "SELECT address, samples, cycles, instructions, branch_misses, branch_instructions, llc_misses, llc_references
                     FROM assembly_address_stats
                     WHERE module_path = ? AND func_name = ?
                     ORDER BY address;",
                )
                .map_err(|err| err.to_string())?;
            stats_stmt
                .bind((1, module_path.as_str()))
                .map_err(|err| err.to_string())?;
            stats_stmt
                .bind((2, func_name.as_str()))
                .map_err(|err| err.to_string())?;

            let mut stats_map = HashMap::new();
            let mut ordered_addresses = Vec::new();
            let mut total_samples = 0u64;
            let mut max_samples = 0u64;

            while let State::Row = stats_stmt.next().map_err(|err| err.to_string())? {
                let address = stats_stmt
                    .read::<i64, _>("address")
                    .map_err(|err| err.to_string())? as u64;
                let samples = stats_stmt
                    .read::<i64, _>("samples")
                    .map_err(|err| err.to_string())? as u64;
                let cycles = stats_stmt
                    .read::<i64, _>("cycles")
                    .map_err(|err| err.to_string())? as u64;
                let instructions = stats_stmt
                    .read::<i64, _>("instructions")
                    .map_err(|err| err.to_string())? as u64;
                let branch_misses = stats_stmt
                    .read::<i64, _>("branch_misses")
                    .map_err(|err| err.to_string())? as u64;
                let branch_instructions = stats_stmt
                    .read::<i64, _>("branch_instructions")
                    .map_err(|err| err.to_string())?
                    as u64;
                let llc_misses = stats_stmt
                    .read::<i64, _>("llc_misses")
                    .map_err(|err| err.to_string())? as u64;
                let llc_references = stats_stmt
                    .read::<i64, _>("llc_references")
                    .map_err(|err| err.to_string())? as u64;

                stats_map.insert(
                    address,
                    (
                        samples,
                        cycles,
                        instructions,
                        branch_misses,
                        branch_instructions,
                        llc_misses,
                        llc_references,
                    ),
                );
                ordered_addresses.push(address);
                total_samples += samples;
                max_samples = max_samples.max(samples);
            }

            if ordered_addresses.is_empty() {
                return Err("No samples recorded for the selected hotspot".to_string());
            }

            let mut load_bias_stmt = conn
                .prepare("SELECT load_bias FROM assembly_module_metadata WHERE module_path = ?")
                .map_err(|err| err.to_string())?;
            load_bias_stmt
                .bind((1, module_path.as_str()))
                .map_err(|err| err.to_string())?;
            let load_bias = match load_bias_stmt.next().map_err(|err| err.to_string())? {
                State::Row => load_bias_stmt
                    .read::<i64, _>(0)
                    .map_err(|err| err.to_string())?,
                State::Done => 0,
            };

            let start_runtime = ordered_addresses.first().copied().unwrap();
            let end_runtime = ordered_addresses.last().copied().unwrap();
            let start_rel = start_runtime as i64 - load_bias;
            let end_rel = end_runtime as i64 - load_bias;

            let mut symbol_lookup = conn
                .prepare(
                    "SELECT symbol
                     FROM assembly_lines
                     WHERE module_path = ?
                       AND rel_address BETWEEN ? AND ?
                       AND symbol IS NOT NULL
                     ORDER BY runtime_address
                     LIMIT 1;",
                )
                .map_err(|err| err.to_string())?;
            symbol_lookup
                .bind((1, module_path.as_str()))
                .map_err(|err| err.to_string())?;
            symbol_lookup
                .bind((2, start_rel - 256))
                .map_err(|err| err.to_string())?;
            symbol_lookup
                .bind((3, end_rel + 256))
                .map_err(|err| err.to_string())?;

            let mut resolved_symbol = match symbol_lookup.next().map_err(|err| err.to_string())? {
                State::Row => symbol_lookup
                    .read::<Option<String>, _>(0)
                    .map_err(|err| err.to_string())?,
                State::Done => None,
            };

            if resolved_symbol.is_none() {
                resolved_symbol = Some(func_name.clone());
            }

            let selected_symbol = resolved_symbol.unwrap();

            let mut lines_stmt = conn
                .prepare(
                    "SELECT runtime_address, instruction
                     FROM assembly_lines
                     WHERE module_path = ? AND symbol = ?
                     ORDER BY runtime_address;",
                )
                .map_err(|err| err.to_string())?;
            lines_stmt
                .bind((1, module_path.as_str()))
                .map_err(|err| err.to_string())?;
            lines_stmt
                .bind((2, selected_symbol.as_str()))
                .map_err(|err| err.to_string())?;

            let mut rows = Vec::new();

            while let State::Row = lines_stmt.next().map_err(|err| err.to_string())? {
                let runtime_address = lines_stmt
                    .read::<i64, _>("runtime_address")
                    .map_err(|err| err.to_string())? as u64;
                let instruction = lines_stmt
                    .read::<String, _>("instruction")
                    .map_err(|err| err.to_string())?;

                let stats = stats_map.get(&runtime_address).copied().unwrap_or_default();
                let (
                    samples,
                    cycles,
                    instructions,
                    branch_misses,
                    branch_instructions,
                    llc_misses,
                    llc_references,
                ) = stats;

                rows.push(AssemblyRow {
                    address: runtime_address,
                    instruction,
                    samples,
                    share: if total_samples > 0 {
                        samples as f64 / total_samples as f64
                    } else {
                        0.0
                    },
                    cycles,
                    instructions,
                    branch_misses,
                    branch_instructions,
                    llc_misses,
                    llc_references,
                });
            }

            if rows.is_empty() {
                let mut fallback_stmt = conn
                    .prepare(
                        "SELECT runtime_address, instruction
                         FROM assembly_lines
                         WHERE module_path = ?
                         ORDER BY runtime_address
                         LIMIT 2048;",
                    )
                    .map_err(|err| err.to_string())?;
                fallback_stmt
                    .bind((1, module_path.as_str()))
                    .map_err(|err| err.to_string())?;

                while let State::Row = fallback_stmt.next().map_err(|err| err.to_string())? {
                    let runtime_address = fallback_stmt
                        .read::<i64, _>("runtime_address")
                        .map_err(|err| err.to_string())?
                        as u64;
                    let instruction = fallback_stmt
                        .read::<String, _>("instruction")
                        .map_err(|err| err.to_string())?;

                    let stats = stats_map.get(&runtime_address).copied().unwrap_or_default();
                    let (
                        samples,
                        cycles,
                        instructions,
                        branch_misses,
                        branch_instructions,
                        llc_misses,
                        llc_references,
                    ) = stats;

                    rows.push(AssemblyRow {
                        address: runtime_address,
                        instruction,
                        samples,
                        share: if total_samples > 0 {
                            samples as f64 / total_samples as f64
                        } else {
                            0.0
                        },
                        cycles,
                        instructions,
                        branch_misses,
                        branch_instructions,
                        llc_misses,
                        llc_references,
                    });
                }
            }

            for (&address, stats) in &stats_map {
                if rows.iter().any(|row| row.address == address) {
                    continue;
                }
                let (
                    samples,
                    cycles,
                    instructions,
                    branch_misses,
                    branch_instructions,
                    llc_misses,
                    llc_references,
                ) = *stats;
                rows.push(AssemblyRow {
                    address,
                    instruction: format!("<no disassembly for 0x{:016x}>", address),
                    samples,
                    share: if total_samples > 0 {
                        samples as f64 / total_samples as f64
                    } else {
                        0.0
                    },
                    cycles,
                    instructions,
                    branch_misses,
                    branch_instructions,
                    llc_misses,
                    llc_references,
                });
            }

            rows.sort_by_key(|row| row.address);

            let symbol = selected_symbol;

            Ok(AssemblyViewState {
                func_name: func_name.clone(),
                module_path,
                symbol,
                rows,
                selected: Some(0),
                offset: 0,
                max_samples,
            })
        })();

        match result {
            Ok(view_state) => {
                let mut state = self.state.lock();
                if state.assembly_request_id != request_id {
                    return;
                }
                state.assembly_loading = false;
                state.assembly_error = None;
                state.assembly = Some(view_state);
            }
            Err(message) => {
                let mut state = self.state.lock();
                if state.assembly_request_id != request_id {
                    return;
                }
                state.assembly_loading = false;
                state.assembly_error = Some(message);
                state.assembly = None;
                state.focus = HotspotsFocus::Assembly;
            }
        }
    }

    fn render_assembly_overlay(
        &self,
        buf: &mut ratatui::prelude::Buffer,
        area: Rect,
        loading: bool,
        error: Option<String>,
        view: Option<AssemblyViewState>,
    ) {
        Clear.render(area, buf);

        let title = view
            .as_ref()
            .map(|v| format!("Assembly - {}", v.func_name))
            .unwrap_or_else(|| "Assembly".to_string());

        let block = Block::bordered().title(title);
        block.render(area, buf);

        let vertical = Layout::vertical_margin(Layout::vertical([Constraint::Fill(1)]), 1);
        let [inner_area] = vertical.areas(area);
        let horizontal = Layout::horizontal_margin(Layout::horizontal([Constraint::Fill(1)]), 1);
        let [inner_area] = horizontal.areas(inner_area);

        if loading {
            Paragraph::new("Loading assembly...")
                .alignment(Alignment::Center)
                .render(inner_area, buf);
            return;
        }

        if let Some(message) = error {
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .render(inner_area, buf);
            return;
        }

        let Some(view) = view else {
            Paragraph::new("Assembly data is not available")
                .alignment(Alignment::Center)
                .render(inner_area, buf);
            return;
        };

        if view.rows.is_empty() {
            Paragraph::new("No disassembly found for the selected function")
                .alignment(Alignment::Center)
                .render(inner_area, buf);
            return;
        }

        let layout = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]);
        let [info_area, table_area] = layout.areas(inner_area);

        let info_lines = vec![
            Line::from(format!("Function: {}", view.func_name)),
            Line::from(format!("Symbol: {}", view.symbol)),
            Line::from(format!("Module: {}", view.module_path)),
        ];
        Paragraph::new(info_lines).render(info_area, buf);

        let header = Row::new(vec![
            Cell::from(""),
            Cell::from("Address"),
            Cell::from("Assembly"),
            Cell::from(Text::from("Samples").alignment(Alignment::Right)),
            Cell::from(Text::from("Share %").alignment(Alignment::Right)),
            Cell::from(Text::from("Cycles").alignment(Alignment::Right)),
            Cell::from(Text::from("Instructions").alignment(Alignment::Right)),
            Cell::from(Text::from("IPC").alignment(Alignment::Right)),
            Cell::from(Text::from("Branch MPKI").alignment(Alignment::Right)),
            Cell::from(Text::from("Branch mispred %").alignment(Alignment::Right)),
            Cell::from(Text::from("Cache MPKI").alignment(Alignment::Right)),
            Cell::from(Text::from("Cache miss %").alignment(Alignment::Right)),
        ])
        .style(Style::new().bold());

        let rows_iter = view.rows.iter().map(|row| {
            let heat_cell = Cell::from("  ").style(heat_style(row.samples, view.max_samples));
            let address = format!("0x{:016x}", row.address);
            let asm_text = row.instruction.clone();
            let samples = row.samples.to_formatted_string(&Locale::en);
            let share = format!("{:.2}", row.share * 100.0);
            let cycles = row.cycles.to_formatted_string(&Locale::en);
            let instructions = row.instructions.to_formatted_string(&Locale::en);
            let ipc = if row.cycles > 0 {
                row.instructions as f64 / row.cycles as f64
            } else {
                0.0
            };
            let branch_mpki = if row.instructions > 0 {
                row.branch_misses as f64 / row.instructions as f64 * 1000.0
            } else {
                0.0
            };
            let branch_miss_pct = if row.branch_instructions > 0 {
                row.branch_misses as f64 / row.branch_instructions as f64 * 100.0
            } else {
                0.0
            };
            let cache_mpki = if row.instructions > 0 {
                row.llc_misses as f64 / row.instructions as f64 * 1000.0
            } else {
                0.0
            };
            let cache_miss_pct = if row.llc_misses + row.llc_references > 0 {
                row.llc_misses as f64 / (row.llc_misses + row.llc_references) as f64 * 100.0
            } else {
                0.0
            };

            Row::new(vec![
                heat_cell,
                Cell::from(address),
                Cell::from(asm_text),
                Cell::from(Text::from(samples).alignment(Alignment::Right)),
                Cell::from(Text::from(share).alignment(Alignment::Right)),
                Cell::from(Text::from(cycles).alignment(Alignment::Right)),
                Cell::from(Text::from(instructions).alignment(Alignment::Right)),
                Cell::from(Text::from(format!("{:.2}", ipc)).alignment(Alignment::Right)),
                Cell::from(Text::from(format!("{:.2}", branch_mpki)).alignment(Alignment::Right)),
                Cell::from(
                    Text::from(format!("{:.2}", branch_miss_pct)).alignment(Alignment::Right),
                ),
                Cell::from(Text::from(format!("{:.2}", cache_mpki)).alignment(Alignment::Right)),
                Cell::from(
                    Text::from(format!("{:.2}", cache_miss_pct)).alignment(Alignment::Right),
                ),
            ])
        });

        let table_rows = rows_iter.collect::<Vec<_>>();

        let widths = vec![
            Constraint::Length(3),
            Constraint::Length(18),
            Constraint::Fill(3),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Length(12),
            Constraint::Length(12),
        ];

        let mut table_state = TableState::default()
            .with_selected(view.selected)
            .with_offset(view.offset);

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("▶ ");

        ratatui::widgets::StatefulWidget::render(table, table_area, buf, &mut table_state);
    }
}

impl HotspotsTab {
    pub fn handle_event(&mut self, code: KeyCode) {
        let hotspots_len = {
            let hotspots = self.hotspots.read();
            hotspots.len()
        };

        let mut state = self.state.lock();

        if state.focus == HotspotsFocus::Assembly {
            match code {
                KeyCode::Esc => {
                    state.assembly_request_id = state.assembly_request_id.wrapping_add(1);
                    state.focus = HotspotsFocus::List;
                    state.assembly = None;
                    state.assembly_loading = false;
                    state.assembly_error = None;
                }
                KeyCode::Enter => {
                    state.assembly_request_id = state.assembly_request_id.wrapping_add(1);
                    state.focus = HotspotsFocus::List;
                    state.assembly = None;
                    state.assembly_loading = false;
                    state.assembly_error = None;
                }
                _ => {
                    if state.assembly_loading {
                        return;
                    }
                    if let Some(assembly) = state.assembly.as_mut() {
                        let len = assembly.rows.len();
                        if len == 0 {
                            return;
                        }
                        match code {
                            KeyCode::Down => {
                                let current = assembly.selected.unwrap_or(0);
                                let next = (current + 1).min(len - 1);
                                assembly.selected = Some(next);
                                if next >= assembly.offset + ASSEMBLY_VIEW_WINDOW_HINT {
                                    assembly.offset = assembly.offset.saturating_add(1);
                                }
                            }
                            KeyCode::Up => {
                                let current = assembly.selected.unwrap_or(0);
                                let next = current.saturating_sub(1);
                                assembly.selected = Some(next);
                                if next < assembly.offset {
                                    assembly.offset = assembly.offset.saturating_sub(1);
                                }
                            }
                            KeyCode::Char('n') => {
                                let start = assembly.selected.unwrap_or(0);
                                if let Some(next_hot) =
                                    find_next_hot_row(&assembly.rows, start, assembly.max_samples)
                                {
                                    assembly.selected = Some(next_hot);
                                    ensure_assembly_row_visible(assembly, next_hot);
                                }
                            }
                            KeyCode::Char('N') => {
                                let start = assembly.selected.unwrap_or(0);
                                if let Some(prev_hot) =
                                    find_prev_hot_row(&assembly.rows, start, assembly.max_samples)
                                {
                                    assembly.selected = Some(prev_hot);
                                    ensure_assembly_row_visible(assembly, prev_hot);
                                }
                            }
                            KeyCode::PageDown => {
                                let current = assembly.selected.unwrap_or(0);
                                let next = (current + ASSEMBLY_SCROLL_STEP).min(len - 1);
                                assembly.selected = Some(next);
                                assembly.offset = (assembly.offset + ASSEMBLY_SCROLL_STEP)
                                    .min(len.saturating_sub(1));
                            }
                            KeyCode::PageUp => {
                                let current = assembly.selected.unwrap_or(0);
                                let next = current.saturating_sub(ASSEMBLY_SCROLL_STEP);
                                assembly.selected = Some(next);
                                assembly.offset =
                                    assembly.offset.saturating_sub(ASSEMBLY_SCROLL_STEP);
                            }
                            KeyCode::Home => {
                                assembly.selected = Some(0);
                                assembly.offset = 0;
                            }
                            KeyCode::End => {
                                assembly.selected = Some(len - 1);
                            }
                            _ => {}
                        }
                    }
                }
            }
            return;
        }

        if hotspots_len == 0 {
            return;
        }

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
            KeyCode::Enter => {
                if let Some(idx) = state.selected {
                    state.focus = HotspotsFocus::Assembly;
                    state.assembly_loading = true;
                    state.assembly_error = None;
                    state.assembly = None;
                    drop(state);
                    let func_name = {
                        let hotspots = self.hotspots.read();
                        hotspots.get(idx).map(|row| row.func_name.clone())
                    };
                    if let Some(func_name) = func_name {
                        self.request_assembly(func_name);
                    } else {
                        let mut state = self.state.lock();
                        state.focus = HotspotsFocus::List;
                        state.assembly_loading = false;
                        state.assembly_error = Some(
                            "Unable to open assembly view for the selected hotspot".to_string(),
                        );
                    }
                    return;
                }
            }
            _ => {}
        }
    }
}

const COLUMN_COUNT: usize = 9;
const STICKY_COLUMN_COUNT: usize = 1;
const METRIC_COLUMN_COUNT: usize = COLUMN_COUNT - STICKY_COLUMN_COUNT;
const ASSEMBLY_VIEW_WINDOW_HINT: usize = 20;
const ASSEMBLY_SCROLL_STEP: usize = 10;
const HOT_LINE_MIN_SAMPLES: u64 = 1;
const HOT_LINE_RATIO_THRESHOLD: f64 = 0.6;

const HEATMAP_GRADIENT: &[(f64, (u8, u8, u8))] = &[
    (0.05, (255, 250, 245)), // almost white beige
    (0.15, (255, 237, 188)), // light yellow
    (0.3, (255, 213, 128)),  // rich yellow
    (0.5, (255, 185, 77)),   // amber
    (0.7, (255, 140, 40)),   // orange
    (1.0, (236, 65, 25)),    // deep red
];

fn heat_ratio(samples: u64, max_samples: u64) -> f64 {
    if max_samples == 0 {
        return 0.0;
    }
    samples as f64 / max_samples as f64
}

fn lerp(a: u8, b: u8, t: f64) -> u8 {
    ((a as f64) + (b as f64 - a as f64) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn gradient_color(samples: u64, max_samples: u64) -> Option<(u8, u8, u8)> {
    if samples == 0 {
        return None;
    }

    let ratio = heat_ratio(samples, max_samples).clamp(0.0, 1.0);

    if HEATMAP_GRADIENT.is_empty() {
        return None;
    }

    for window in HEATMAP_GRADIENT.windows(2) {
        let (start_ratio, start_color) = window[0];
        let (end_ratio, end_color) = window[1];

        if ratio <= end_ratio {
            let span = (ratio - start_ratio).max(0.0) / (end_ratio - start_ratio).max(f64::EPSILON);
            let r = lerp(start_color.0, end_color.0, span);
            let g = lerp(start_color.1, end_color.1, span);
            let b = lerp(start_color.2, end_color.2, span);
            return Some((r, g, b));
        }
    }

    HEATMAP_GRADIENT.last().map(|(_, color)| *color)
}

fn contrast_text_color(r: u8, g: u8, b: u8) -> Color {
    let luma = 0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64;
    if luma > 160.0 {
        Color::Rgb(35, 35, 35)
    } else {
        Color::Rgb(250, 250, 250)
    }
}

fn ensure_assembly_row_visible(assembly: &mut AssemblyViewState, index: usize) {
    if assembly.rows.is_empty() {
        assembly.offset = 0;
        return;
    }

    if index < assembly.offset {
        assembly.offset = index;
    } else {
        let window = ASSEMBLY_VIEW_WINDOW_HINT.max(1);
        let visible_end = assembly.offset.saturating_add(window);
        if index >= visible_end {
            let desired_offset = index.saturating_add(1).saturating_sub(window);
            let max_offset = assembly.rows.len().saturating_sub(1);
            assembly.offset = desired_offset.min(max_offset);
        }
    }

    let max_offset = assembly.rows.len().saturating_sub(1);
    if assembly.offset > max_offset {
        assembly.offset = max_offset;
    }
}

fn find_next_hot_row(rows: &[AssemblyRow], start: usize, max_samples: u64) -> Option<usize> {
    if rows.is_empty() || max_samples == 0 {
        return None;
    }

    let len = rows.len();
    let mut idx = start.saturating_add(1);
    while idx < len {
        let samples = rows[idx].samples;
        if samples < HOT_LINE_MIN_SAMPLES {
            idx += 1;
            continue;
        }

        if heat_ratio(samples, max_samples) >= HOT_LINE_RATIO_THRESHOLD {
            return Some(idx);
        }
        idx += 1;
    }

    None
}

fn find_prev_hot_row(rows: &[AssemblyRow], start: usize, max_samples: u64) -> Option<usize> {
    if rows.is_empty() || max_samples == 0 {
        return None;
    }

    if start == 0 {
        return None;
    }

    let mut idx = start.saturating_sub(1);
    loop {
        let samples = rows[idx].samples;
        if samples >= HOT_LINE_MIN_SAMPLES
            && heat_ratio(samples, max_samples) >= HOT_LINE_RATIO_THRESHOLD
        {
            return Some(idx);
        }

        if idx == 0 {
            break;
        }
        idx -= 1;
    }

    None
}

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

fn heat_style(samples: u64, max_samples: u64) -> Style {
    match gradient_color(samples, max_samples) {
        Some((r, g, b)) => Style::default()
            .bg(Color::Rgb(r, g, b))
            .fg(contrast_text_color(r, g, b)),
        None => Style::default(),
    }
}
