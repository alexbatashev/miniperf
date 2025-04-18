#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashSet;
use std::{collections::HashMap, iter::Extend, path::Path, sync::Arc};

use mperf_data::{Event, IString, ProcMap, ProcMapEntry};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use proc_maps::{get_process_maps, Pid};
use thread_local::ThreadLocal;
use tokio::{
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};

pub struct EventDispatcher {
    strings: RwLock<HashMap<String, u64>>,
    proc_maps: RwLock<HashSet<u32>>,
    last_unique_id: ThreadLocal<RefCell<u64>>,
    events_tx: Sender<Event>,
    string_tx: Sender<(u64, String)>,
    proc_map_tx: Sender<u32>,
}

pub struct DispatcherJoinHandle {
    events_worker: JoinHandle<()>,
    string_worker: JoinHandle<()>,
    proc_map_worker: JoinHandle<()>,
}

impl EventDispatcher {
    pub fn new(output_directory: &Path) -> (Arc<Self>, DispatcherJoinHandle) {
        let (events_tx, mut event_rx) = mpsc::channel::<Event>(8192);
        let (string_tx, mut string_rx) = mpsc::channel(8192);
        let (proc_map_tx, mut proc_map_rx) = mpsc::channel::<u32>(8192);

        let events_out_dir = output_directory.to_owned();
        let events_worker = tokio::spawn(async move {
            let mut events_file = std::io::BufWriter::new(
                std::fs::File::create(events_out_dir.join("events.bin"))
                    .expect("event file stream creation"),
            );
            while let Some(event) = event_rx.recv().await {
                let result = event.write_binary(&mut events_file);
                if result.is_err() {
                    eprintln!("Failed to write data for event id {}", event.unique_id);
                }
            }
        });

        let string_out_dir = output_directory.to_owned();
        let string_worker = tokio::spawn(async move {
            let mut strings = vec![];
            while let Some((id, value)) = string_rx.recv().await {
                let string = IString { id, value };
                strings.push(string);
            }

            let mut strings_file =
                std::fs::File::create(string_out_dir.join("strings.json")).expect("strings");
            serde_json::to_writer(&mut strings_file, &strings).expect("failed to write strings");
        });

        let proc_map_out_dir = output_directory.to_owned();
        let proc_map_worker = tokio::spawn(async move {
            let mut proc_map_entries = HashMap::<u32, HashSet<ProcMapEntry>>::new();
            while let Some(pid) = proc_map_rx.recv().await {
                if let Ok(maps) = get_process_maps(pid as Pid) {
                    let pm = maps.iter().filter_map(|m| {
                        if m.filename().is_none() || m.filename().unwrap().ends_with("mperf") {
                            return None;
                        }
                        Some(ProcMapEntry {
                            filename: m
                                .filename()
                                .map(|p| p.to_str().unwrap_or("unknown").to_owned())
                                .unwrap_or("unknown".to_string()),
                            address: m.start(),
                            size: m.size(),
                        })
                    });

                    proc_map_entries.entry(pid).or_default();

                    let set = proc_map_entries.get_mut(&pid).unwrap();

                    set.extend(pm.into_iter());
                }
            }

            let proc_map = proc_map_entries
                .into_iter()
                .map(ProcMap::new)
                .collect::<Vec<_>>();
            let mut map_file =
                std::fs::File::create(proc_map_out_dir.join("proc_map.json")).expect("proc map");
            serde_json::to_writer(&mut map_file, &proc_map).expect("failed to write proc maps");
        });

        (
            Arc::new(EventDispatcher {
                strings: RwLock::new(HashMap::new()),
                proc_maps: RwLock::new(HashSet::new()),
                last_unique_id: ThreadLocal::new(),
                events_tx,
                string_tx,
                proc_map_tx,
            }),
            DispatcherJoinHandle {
                events_worker,
                string_worker,
                proc_map_worker,
            },
        )
    }

    pub fn unique_id(&self) -> u128 {
        let mut counter = self.last_unique_id.get_or(|| RefCell::new(0)).borrow_mut();
        let id = ((std::process::id() as u128) << 96)
            | ((unsafe { libc::gettid() } as u128) << 64)
            | (*counter as u128);
        *counter += 1;
        id
    }

    pub fn string_id(&self, string: &str) -> u64 {
        let strings = self.strings.upgradable_read();

        if strings.contains_key(string) {
            return *strings.get(string).unwrap();
        }

        let id;
        {
            let mut strings = RwLockUpgradableReadGuard::upgrade(strings);

            id = strings.len() as u64;
            strings.insert(string.to_string(), id);
        }

        if let Err(err) = self.string_tx.blocking_send((id, string.to_string())) {
            eprintln!("Lost string {} -> {}: {}", id, string, err);
        }

        id
    }

    pub async fn string_id_async(&self, string: &str) -> u64 {
        let id;

        {
            let strings = self.strings.upgradable_read();

            if strings.contains_key(string) {
                return *strings.get(string).unwrap();
            }

            {
                let mut strings = RwLockUpgradableReadGuard::upgrade(strings);

                id = strings.len() as u64;
                strings.insert(string.to_string(), id);
            }
        }

        if let Err(err) = self.string_tx.send((id, string.to_string())).await {
            eprintln!("Lost string {} -> {}: {}", id, string, err);
        }

        id
    }

    pub fn publish_event_sync(&self, evt: Event) {
        let pid = evt.process_id;
        if let Err(err) = self.events_tx.blocking_send(evt) {
            eprintln!("lost event: {:?}", err);
        }

        if pid == 0 {
            return;
        }

        // FIXME figure out a way to limit traffic here
        if let Err(err) = self.proc_map_tx.blocking_send(pid) {
            eprintln!("lost process map for pid '{}': {:?}", pid, err);
        }
    }

    pub async fn publish_event(&self, evt: Event) {
        let pid = evt.process_id;
        if let Err(err) = self.events_tx.send(evt).await {
            eprintln!("lost event: {:?}", err);
        }

        if pid == 0 {
            return;
        }

        // FIXME figure out a way to limit traffic here
        if let Err(err) = self.proc_map_tx.send(pid).await {
            eprintln!("lost process map for pid '{}': {:?}", pid, err);
        }
    }
}

impl DispatcherJoinHandle {
    pub async fn join(self) {
        let _ = tokio::join!(self.events_worker, self.string_worker, self.proc_map_worker);
    }
}
