use std::collections::HashSet;
use std::{collections::HashMap, io::Write, path::Path, sync::Arc};

use atomic_counter::{AtomicCounter, ConsistentCounter};
use mperf_data::{Event, IString, ProcMap, ProcMapEntry};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use proc_maps::{get_process_maps, Pid};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::{
    fs::File,
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};

pub struct EventDispatcher {
    strings: RwLock<HashMap<String, u64>>,
    proc_maps: RwLock<HashSet<u32>>,
    last_unique_id: ConsistentCounter,
    events_tx: Sender<Event>,
    string_tx: Sender<(u64, String)>,
    proc_map_tx: Sender<u32>,
}

pub struct DispatcherJoinHandle {
    token: watch::Sender<bool>,
    events_worker: JoinHandle<()>,
    string_worker: JoinHandle<()>,
    proc_map_worker: JoinHandle<()>,
}

impl EventDispatcher {
    pub fn new(output_directory: &Path) -> (Arc<Self>, DispatcherJoinHandle) {
        let (events_tx, mut event_rx) = mpsc::channel(8192);
        let (string_tx, mut string_rx) = mpsc::channel(8192);
        let (proc_map_tx, mut proc_map_rx) = mpsc::channel::<u32>(8192);

        let (cancel_tx, cancel_rx) = watch::channel(false);

        let mut events_token = cancel_rx.clone();
        let events_out_dir = output_directory.to_owned();
        let events_worker = tokio::spawn(async move {
            let mut events_file = File::create(events_out_dir.join("events.bin"))
                .await
                .expect("event file stream creation");
            loop {
                tokio::select! {
                    _ = events_token.changed() => {
                        break;
                    }
                    Some(event) = event_rx.recv() => {
                        let data = unsafe {
                            std::slice::from_raw_parts(&event as *const Event as *const u8, std::mem::size_of::<Event>())
                        };
                        events_file.write_all(data).await.expect("write failed");
                    }
                }
            }
        });

        let mut string_token = cancel_rx.clone();
        let string_out_dir = output_directory.to_owned();
        let string_worker = tokio::spawn(async move {
            let mut strings_file =
                std::fs::File::create(string_out_dir.join("strings.jsonl")).expect("strings");

            loop {
                tokio::select! {
                    _ = string_token.changed() => {
                        break;
                    }
                    Some((id, value)) = string_rx.recv() => {
                        let string = IString{id, value};
                        serde_json::to_writer(&mut strings_file, &string).expect("fail");
                        writeln!(&mut strings_file).expect("fail");
                    }
                }
            }
        });

        let mut proc_map_token = cancel_rx.clone();
        let proc_map_out_dir = output_directory.to_owned();
        let proc_map_worker = tokio::spawn(async move {
            let mut map_file =
                std::fs::File::create(proc_map_out_dir.join("proc_map.jsonl")).expect("proc map");
            loop {
                tokio::select! {
                    _ = proc_map_token.changed() => {
                        break;
                    }
                    Some(pid) = proc_map_rx.recv() => {
                        let maps = get_process_maps(pid as Pid).expect("get proc maps");
                        let proc_map_entries = maps.iter().map(|m| ProcMapEntry {
                            filename: m.filename().map(|p| p.to_str().unwrap_or("unknown").to_owned()).unwrap_or("unknown".to_string()),
                            address: m.start(),
                            size: m.size(),
                        }).collect();

                        let proc_map = ProcMap {pid, entries: proc_map_entries};
                        serde_json::to_writer(&mut map_file, &proc_map).expect("fail");
                    }
                }
            }
        });

        (
            Arc::new(EventDispatcher {
                strings: RwLock::new(HashMap::new()),
                proc_maps: RwLock::new(HashSet::new()),
                last_unique_id: ConsistentCounter::new(0),
                events_tx,
                string_tx,
                proc_map_tx,
            }),
            DispatcherJoinHandle {
                token: cancel_tx,
                events_worker,
                string_worker,
                proc_map_worker,
            },
        )
    }

    pub fn unique_id(&self) -> u64 {
        self.last_unique_id.inc() as u64
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

    pub fn publish_event_sync(&self, evt: Event) {
        let pid = evt.process_id;
        if let Err(err) = self.events_tx.blocking_send(evt) {
            eprintln!("lost event: {:?}", err);
        }

        if pid == 0 {
            return;
        }

        let pids = self.proc_maps.upgradable_read();
        if pids.contains(&pid) {
            return;
        }

        {
            let mut pids = RwLockUpgradableReadGuard::upgrade(pids);

            pids.insert(pid);
        }

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

        {
            let pids = self.proc_maps.upgradable_read();
            if pids.contains(&pid) {
                return;
            }

            {
                let mut pids = RwLockUpgradableReadGuard::upgrade(pids);

                pids.insert(pid);
            }
        }

        if let Err(err) = self.proc_map_tx.send(pid).await {
            eprintln!("lost process map for pid '{}': {:?}", pid, err);
        }
    }
}

impl DispatcherJoinHandle {
    pub async fn join(self) {
        let _ = self.token.send(true);
        let _ = tokio::join!(self.events_worker, self.string_worker, self.proc_map_worker);
    }
}
