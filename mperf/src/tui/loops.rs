use std::{collections::HashMap, path::PathBuf, sync::Arc};

use memmap2::{Advice, Mmap};
use mperf_data::{CallFrame, Event, EventType, IString, ProcMap};
use parking_lot::RwLock;
use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    widgets::{Block, Cell, Gauge, Row, Table, Widget},
};
use smallvec::{smallvec, SmallVec};
use tokio::fs::File;

use crate::utils;

#[derive(Clone)]
pub struct LoopsTab {
    res_dir: PathBuf,
    hotspots: Arc<RwLock<Vec<Loop>>>,
    counter: Arc<RwLock<u16>>,
    is_running: Arc<RwLock<bool>>,
}

struct Loop {
    loc: ResolvedLocation,
    avg_flops32: f32,
    avg_flops64: f32,
    avg_bandwidth: f32,
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct ResolvedLocation {
    function_name: String,
    file_name: String,
    line: u32,
}

#[derive(Default, Debug, Clone, Copy)]
struct LoopStat {
    bytes_load: u64,
    bytes_store: u64,
    scalar_int_ops: u64,
    scalar_float_ops: u64,
    scalar_double_ops: u64,
    vector_int_ops: u64,
    vector_float_ops: u64,
    vector_double_ops: u64,
}

impl Widget for LoopsTab {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let hotspots = self.hotspots.read();

        if hotspots.is_empty() {
            let counter = self.counter.read();
            let pb = Gauge::default()
                .block(Block::bordered().title("Loading data..."))
                .gauge_style(Style::new().white().on_black().italic())
                .percent(*counter);
            pb.render(area, buf);
            return;
        }

        let header = [
            Cell::from("Function"),
            Cell::from("Location"),
            Cell::from("Avg. Bandwidth"),
            Cell::from("Avg. FLOPs"),
            Cell::from("Avg. DFLOPs"),
        ]
        .into_iter()
        .collect::<Row>()
        .style(Style::new().bold())
        .height(2);

        let rows = hotspots.iter().map(|loop_| {
            [
                Cell::from(loop_.loc.function_name.as_str()),
                Cell::from(format!("{}:{}", loop_.loc.file_name, loop_.loc.line)),
                Cell::from(format!("{:.2}", loop_.avg_bandwidth)),
                Cell::from(format!("{:.2}", loop_.avg_flops32)),
                Cell::from(format!("{:.2}", loop_.avg_flops64)),
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
    pub fn new(res_dir: PathBuf) -> Self {
        LoopsTab {
            res_dir,
            hotspots: Arc::new(RwLock::new(Vec::new())),
            counter: Arc::new(RwLock::new(0)),
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

        let strings_file = std::fs::File::open(self.res_dir.join("strings.json"))
            .expect("failed to open strings.json");
        let strings: Vec<IString> =
            serde_json::from_reader(strings_file).expect("failed to parse strings.json");

        let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

        let mut loop_data = HashMap::<u128, LoopStat>::new();
        let mut loop_location = HashMap::<u128, ResolvedLocation>::new();
        let mut functions = HashMap::<String, u64>::new();

        let mut cursor = std::io::Cursor::new(data_stream);

        while (cursor.position() as usize) < map.len() {
            let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");

            {
                let mut cntr = self.counter.write();
                *cntr = (100 * cursor.position() / data_stream.len() as u64) as u16;
            }

            if !(evt.ty.is_roofline() || evt.ty == EventType::PmuCycles) {
                continue;
            }

            match evt.ty {
                EventType::PmuCycles => {
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

                    functions
                        .entry(sym_name)
                        .and_modify(|v| *v += evt.value)
                        .or_insert(evt.value);
                }
                EventType::RooflineLoopStart => {
                    loop_data.insert(
                        evt.unique_id,
                        LoopStat {
                            ..Default::default()
                        },
                    );

                    if let CallFrame::Location(loc) = evt.callstack[0] {
                        let file_name = strings
                            .iter()
                            .find_map(|s| {
                                if s.id == (loc.file_name as u64) {
                                    Some(s.value.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("unknown".to_string());
                        let function_name = strings
                            .iter()
                            .find_map(|s| {
                                if s.id == (loc.function_name as u64) {
                                    Some(s.value.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("unknown".to_string());
                        loop_location.insert(
                            evt.unique_id,
                            ResolvedLocation {
                                function_name,
                                file_name,
                                line: loc.line,
                            },
                        );
                    }
                }
                EventType::RooflineLoopEnd => {}
                EventType::RooflineBytesLoad => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.bytes_load = evt.value;
                }
                EventType::RooflineBytesStore => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.bytes_store = evt.value;
                }
                EventType::RooflineScalarIntOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.scalar_int_ops = evt.value;
                }
                EventType::RooflineScalarFloatOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.scalar_float_ops = evt.value;
                }
                EventType::RooflineScalarDoubleOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.scalar_double_ops = evt.value;
                }
                EventType::RooflineVectorIntOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.vector_int_ops = evt.value;
                }
                EventType::RooflineVectorFloatOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.vector_float_ops = evt.value;
                }
                EventType::RooflineVectorDoubleOps => {
                    let stats = loop_data
                        .get_mut(&evt.parent_id)
                        .expect("Loop start event not found!!!");
                    stats.vector_double_ops = evt.value;
                }
                _ => panic!("Unsupported roofline event '{:?}'", evt.ty),
            }
        }

        let mut reverse_loop_ids = HashMap::<ResolvedLocation, SmallVec<[u128; 32]>>::new();

        for (id, loc) in loop_location.iter() {
            reverse_loop_ids
                .entry(loc.clone())
                .and_modify(|ids| ids.push(*id))
                .or_insert(smallvec![*id]);
        }

        let mut loops = vec![];

        for (loc, ids) in reverse_loop_ids.iter() {
            let mut flops32 = vec![];
            let mut flops64 = vec![];
            let mut bandwidth = vec![];

            for id in ids {
                let stats = loop_data.get(id).unwrap();
                // FIXME: these are some artificial scaling factors for vector ops.
                flops32.push((stats.scalar_float_ops + 8 * stats.vector_float_ops) as f32);
                flops64.push((stats.scalar_double_ops + 4 * stats.vector_double_ops) as f32);
                bandwidth.push((stats.bytes_load + stats.bytes_store) as f32);
            }

            loops.push(Loop {
                loc: loc.clone(),
                avg_flops32: flops32.iter().sum::<f32>() / flops32.len() as f32,
                avg_flops64: flops64.iter().sum::<f32>() / flops64.len() as f32,
                avg_bandwidth: bandwidth.iter().sum::<f32>() / bandwidth.len() as f32,
            })
        }

        loops.sort_by_cached_key(|loop_| {
            functions
                .get(&loop_.loc.function_name)
                .cloned()
                .unwrap_or_default()
        });

        let mut hotspots = self.hotspots.write();
        *hotspots = loops;
        *self.is_running.write() = false;
    }
}
