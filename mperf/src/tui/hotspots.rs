use std::{collections::HashMap, path::PathBuf, sync::Arc};

use memmap2::{Advice, Mmap};
use mperf_data::{Event, EventType, RecordInfo};
use parking_lot::RwLock;
use ratatui::{
    layout::Constraint,
    widgets::{Cell, Row, Table, Widget},
};
use tokio::fs::File;

#[derive(Clone)]
pub struct HotspotsTab {
    res_dir: PathBuf,
    record_info: RecordInfo,
    hotspots: Arc<RwLock<Vec<Hotspot>>>,
    counter: Arc<RwLock<u16>>,
    is_running: Arc<RwLock<bool>>,
}

struct Hotspot {
    name: String,
}

impl Widget for HotspotsTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let header = ["Function", "Cycles", "Instructions", "IPC"]
            .into_iter()
            .map(Cell::from)
            .collect::<Row>()
            .height(1);

        let rows = self.hotspots.read().iter().map(|h| {
            let cols = [Cell::new(h.name.clone()), Cell::new("0"), Cell::new("0"), Cell::new("0")];
            cols.into_iter().collect::<Row>() 
        }).collect::<Vec<_>>();

        let t = Table::new(rows, [Constraint::Min(10)]).header(header);

        t.render(area, buf);
    }
}

impl HotspotsTab {
    pub fn new(res_dir: PathBuf, record_info: RecordInfo) -> Self {
        HotspotsTab {
            res_dir,
            record_info,
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

        let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

        let mut cursor = std::io::Cursor::new(data_stream);

        let mut hostpots_tmp = HashMap::<String, HashMap<EventType, u64>>::new();

        while (cursor.position() as usize) < map.len() {
            // FIXME: should we just skip?
            let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

            {
                let mut cntr = self.counter.write();
                *cntr = (100 * cursor.position() / data_stream.len() as u64) as u16;
            }
        }

        let mut hotspots = self.hotspots.write();
        // hotspots.clear();
        hotspots.push(Hotspot { name: "TEST".to_string() });
        *self.is_running.write() = false;
    }
}
