use std::{collections::HashMap, path::Path, sync::Arc};

use atomic_counter::{AtomicCounter, ConsistentCounter};
use mperf_data::{Event, IString};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::{
    fs::File,
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};

pub struct EventDispatcher {
    strings: RwLock<HashMap<String, u64>>,
    last_unique_id: ConsistentCounter,
    events_tx: Sender<Event>,
    string_tx: Sender<(u64, String)>,
}

pub struct DispatcherJoinHandle {
    token: watch::Sender<bool>,
    events_worker: JoinHandle<()>,
    string_worker: JoinHandle<()>,
}

impl EventDispatcher {
    pub fn new(output_directory: &Path) -> (Arc<Self>, DispatcherJoinHandle) {
        let (events_tx, mut event_rx) = mpsc::channel(8192);
        let (string_tx, mut string_rx) = mpsc::channel(8192);

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
            let mut strings = vec![];

            loop {
                tokio::select! {
                    _ = string_token.changed() => {
                        break;
                    }
                    Some((id, value)) = string_rx.recv() => {
                        strings.push(IString{id, value});
                    }
                }
            }

            let mut strings_file =
                std::fs::File::create(string_out_dir.join("strings.json")).expect("strings");
            serde_json::to_writer(&mut strings_file, &strings).expect("fail");
        });

        (
            Arc::new(EventDispatcher {
                strings: RwLock::new(HashMap::new()),
                last_unique_id: ConsistentCounter::new(0),
                events_tx,
                string_tx,
            }),
            DispatcherJoinHandle {
                token: cancel_tx,
                events_worker,
                string_worker,
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

    pub fn publish_event(&self, evt: Event) {
        if let Err(err) = self.events_tx.blocking_send(evt) {
            eprintln!("lost event: {:?}", err);
        }
    }
}

impl DispatcherJoinHandle {
    pub async fn join(self) {
        let _ = self.token.send(true);
        let _ = tokio::join!(self.events_worker, self.string_worker);
    }
}
