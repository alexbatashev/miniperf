use std::{collections::HashMap, path::PathBuf, sync::Arc};

use memmap2::{Advice, Mmap};
use mperf_data::{CallFrame, Event, EventType, ProcMap};
use num_format::{Locale, ToFormattedString};
use parking_lot::RwLock;
use ratatui::{
    layout::{Alignment, Constraint},
    style::{Style, Stylize},
    text::Text,
    widgets::{Block, Cell, Row, Table, Widget},
};
use tokio::fs::File;

use crate::utils::{self, get_event_readable_name};

#[derive(Clone)]
pub struct HotspotsTab {
    res_dir: PathBuf,
    hotspots: Arc<RwLock<Vec<Hotspot>>>,
    counter: Arc<RwLock<u16>>,
    is_running: Arc<RwLock<bool>>,
}

#[derive(Default, Debug, Clone)]
struct CounterData {
    name: String,
    value: u64,
}

struct Hotspot {
    name: String,
    counters: Vec<(EventType, CounterData)>,
}

impl Widget for HotspotsTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let hotspots = self.hotspots.read();
        let header = get_column_names(&hotspots);

        if header.is_none() {
            return;
        }

        let (rows, widths) = get_rows(&hotspots);

        let t = Table::new(rows, widths)
            .header(header.unwrap())
            .block(Block::bordered());

        t.render(area, buf);
    }
}

impl HotspotsTab {
    pub fn new(res_dir: PathBuf) -> Self {
        HotspotsTab {
            res_dir,
            hotspots: Arc::new(RwLock::new(Vec::new())),
            counter: Arc::new(RwLock::new(80)),
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
        let file = File::open(self.res_dir.join("events.bin"))
            .await
            .expect("failed to open events.bin");

        let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
        map.advise(Advice::Sequential)
            .expect("Failed to advice sequential reads");

        let proc_map_file = std::fs::File::open(self.res_dir.join("proc_map.json"))
            .expect("failed to open proc_map.json");
        let proc_map: Vec<ProcMap> =
            serde_json::from_reader(proc_map_file).expect("failed to parse proc_map.json");

        let resolved_pm = utils::resolve_proc_maps(&proc_map);

        let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

        let mut cursor = std::io::Cursor::new(data_stream);

        let mut hotspots_tmp = HashMap::<String, HashMap<EventType, CounterData>>::new();

        while (cursor.position() as usize) < map.len() {
            // FIXME: should we just skip?
            let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

            {
                let mut cntr = self.counter.write();
                *cntr = (100 * cursor.position() / data_stream.len() as u64) as u16;
            }

            if !evt.ty.is_pmu() && !evt.ty.is_os() {
                continue;
            }

            let pm = resolved_pm.get(&evt.process_id);
            if pm.is_none() {
                continue;
            }

            let pm = pm.unwrap();

            let sym_name = match evt.callstack[0] {
                CallFrame::IP(ip) => {
                    utils::find_sym_name(pm, ip as usize).unwrap_or("[unknown]".to_string())
                }
                CallFrame::Location(_) => "[unknown]".to_string(),
            };

            let counters = if let Some(counters) = hotspots_tmp.get_mut(&sym_name) {
                counters
            } else {
                hotspots_tmp.insert(sym_name.clone(), HashMap::new());
                hotspots_tmp.get_mut(&sym_name).unwrap()
            };

            let value =
                (evt.value as f64 * (evt.time_enabled as f64 / evt.time_running as f64)) as u64;
            if let Some(cntr) = counters.get_mut(&evt.ty) {
                cntr.value += value;
            } else {
                counters.insert(
                    evt.ty,
                    CounterData {
                        name: get_event_readable_name(&evt),
                        value,
                    },
                );
            }
        }

        let mut hotspots = self.hotspots.write();
        for (k, v) in hotspots_tmp.into_iter() {
            let mut counters = v.into_iter().collect::<Vec<_>>();
            counters.sort_unstable_by_key(|(t, _)| *t);
            hotspots.push(Hotspot { name: k, counters })
        }
        hotspots.sort_by_cached_key(|h| std::cmp::Reverse(h.counters[0].1.value));
        *self.is_running.write() = false;
    }
}

fn get_column_names(hotspots: &[Hotspot]) -> Option<Row<'_>> {
    if hotspots.is_empty() {
        return None;
    }

    Some(
        [Cell::from("Function")]
            .into_iter()
            .chain(
                hotspots[0]
                    .counters
                    .iter()
                    .map(|(_, v)| &v.name)
                    .cloned()
                    .map(|e| Cell::from(Text::from(e).alignment(Alignment::Right))),
            )
            .collect::<Row>()
            .style(Style::new().bold())
            .height(2),
    )
}

fn get_rows(hotspots: &[Hotspot]) -> (Vec<Row<'_>>, Vec<Constraint>) {
    let rows = hotspots
        .iter()
        .map(|h| {
            [Cell::new(h.name.clone())]
                .into_iter()
                .chain(h.counters.iter().map(|(_, c)| {
                    Cell::from(
                        Text::from(c.value.to_formatted_string(&Locale::en))
                            .alignment(Alignment::Right),
                    )
                }))
                .collect::<Row>()
        })
        .collect::<Vec<_>>();

    let widths = [Constraint::Max(30)]
        .into_iter()
        .chain(hotspots[0].counters.iter().map(|_| Constraint::Max(20)))
        .collect();

    (rows, widths)
}
