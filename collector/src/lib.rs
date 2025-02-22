use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use futures::AsyncReadExt;
use lazy_static::lazy_static;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Mutex},
};
use tokio::task::LocalSet;

use mperf_data::{Event, IPCMessage, IPCString};

pub mod ffi;

lazy_static! {
    static ref SENDER: Mutex<Arc<dyn Sender>> = {
        let path = std::env::var("MPERF_COLLECTOR_ADDR")
            .expect("mperf must set MPERF_COLLECTOR_ADDR env variable");
        println!("Connect to addr {}", path);
        let sender = UnixSender::new(path);
        Mutex::new(Arc::new(sender))
    };
    static ref STRINGS: RwLock<HashMap<String, u64>> = RwLock::new(HashMap::new());
    static ref PROFILING_ENABLED: bool = std::env::var("MPERF_COLLECTOR_ENABLED").is_ok();
    static ref ROOFLINE_INSTR_ENABLED: bool =
        std::env::var("MPERF_COLLECTOR_ROOFLINE_INSTRUMENTED").is_ok();
}

thread_local! {
    static LAST_ID: RefCell<u64> = const { RefCell::new(0) };
}

pub fn send_event(evt: Event) -> Result<(), Box<dyn std::error::Error>> {
    let sender = SENDER.lock()?;
    sender.send(IPCMessage::Event(evt));

    Ok(())
}

pub fn get_string_id(string: &str) -> u64 {
    let reader = STRINGS.upgradable_read();
    if reader.contains_key(string) {
        return *reader.get(string).unwrap();
    }

    let hash = {
        let mut writer = RwLockUpgradableReadGuard::upgrade(reader);

        // We now have exclusive lock, double check no one has added our string
        if writer.contains_key(string) {
            return *writer.get(string).unwrap();
        }

        let mut hasher = DefaultHasher::new();
        string.hash(&mut hasher);
        let hash = hasher.finish();

        writer.insert(string.to_string(), hash);

        hash
    };

    let sender = SENDER.lock().unwrap();
    sender.send(IPCMessage::String(IPCString {
        key: hash,
        value: string.to_string(),
    }));

    println!("SENT A STRING!!!!!!!!!");

    hash
}

pub fn get_next_id() -> u128 {
    let counter = LAST_ID.with_borrow_mut(|cnt| {
        let last = *cnt;
        *cnt += 1;
        last as u128
    });

    ((std::process::id() as u128) << 96) | ((unsafe { libc::gettid() as u128 }) << 64) | counter
}

pub fn profiling_enabled() -> bool {
    *PROFILING_ENABLED
}

pub fn roofline_instrumentation_enabled() -> bool {
    *ROOFLINE_INSTR_ENABLED
}

trait Sender: Sync + Send {
    fn send(&self, message: IPCMessage);
}

struct UnixSender {
    sender: tokio::sync::mpsc::Sender<IPCMessage>,
    cancel_tx: tokio::sync::watch::Sender<bool>,
}

impl UnixSender {
    fn new(path: String) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_io()
                .build()
                .expect("Failed to create tokio runtime"),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<IPCMessage>(1024);
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);

        std::thread::spawn(move || {
            let local = LocalSet::new();
            local.spawn_local(async move {
                let stream = tokio::net::UnixStream::connect(path).await.unwrap();
                let (reader, writer) =
                    tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
                let rpc_network = Box::new(twoparty::VatNetwork::new(
                    futures::io::BufReader::new(reader),
                    futures::io::BufWriter::new(writer),
                    rpc_twoparty_capnp::Side::Client,
                    Default::default(),
                ));

                let mut rpc_system = RpcSystem::new(rpc_network, None);

                let ipc: mperf_data::IPCClient =
                    rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

                tokio::task::spawn_local(rpc_system);

                loop {
                    tokio::select! {
                        _ = cancel_rx.changed() => {
                            break;
                        }
                        Some(message) = rx.recv() => {
                            let mut request = ipc.post_request();
                            request.get().init_message().set_message(&message);
                            let _ = request.send();
                        }
                    }
                }
            });
            runtime.block_on(local);
        });

        UnixSender {
            sender: tx,
            cancel_tx,
        }
    }
}

impl Sender for UnixSender {
    fn send(&self, message: IPCMessage) {
        let res = self.sender.blocking_send(message);
        if res.is_err() {
            eprintln!("miniperf event send error : {}", res.err().unwrap());
        }
    }
}

impl Drop for UnixSender {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(false);
    }
}
