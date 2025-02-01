use std::{process, thread};

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use futures::AsyncReadExt;
use mperf_data::{EventType, IPCMessage, IPCServer};
use pmu::Counter;
use tokio::{net::UnixListener, task::LocalSet};
use tokio_util::compat::TokioAsyncReadCompatExt;

pub fn counter_to_event_ty(counter: &Counter) -> EventType {
    match counter {
        Counter::Cycles => EventType::PmuCycles,
        Counter::Instructions => EventType::PmuInstructions,
        Counter::LLCReferences => EventType::PmuLlcReferences,
        Counter::LLCMisses => EventType::PmuLlcMisses,
        Counter::BranchInstructions => EventType::PmuBranchInstructions,
        Counter::BranchMisses => EventType::PmuBranchMisses,
        Counter::StalledCyclesFrontend => EventType::PmuStalledCyclesFrontend,
        Counter::StalledCyclesBackend => EventType::PmuStalledCyclesBackend,
        Counter::CpuClock => EventType::OsCpuClock,
        Counter::PageFaults => EventType::OsPageFaults,
        Counter::CpuMigrations => EventType::OsCpuMigrations,
        Counter::ContextSwitches => EventType::OsContextSwitches,
        Counter::Custom(_) => EventType::PmuCustom,
        Counter::Internal {
            name: _,
            desc: _,
            code: _,
        } => EventType::PmuCustom,
    }
}

pub fn create_ipc_server<F: Fn(IPCMessage) + Send + 'static>(callback: F) -> String {
    let path = format!("/tmp/{}.sock", process::id());

    let boxed_callback = Box::new(callback);

    let task_path = path.clone();
    thread::spawn(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = LocalSet::new();
        local.spawn_local(async move {
            let listener = UnixListener::bind(task_path).expect("Could not create unix socket");
            let client: mperf_data::IPCClient =
                capnp_rpc::new_client(IPCServer::new(boxed_callback));

            loop {
                let (stream, _) = listener.accept().await.expect("failed to accept a stream");
                let (reader, writer) = TokioAsyncReadCompatExt::compat(stream).split();

                let network = twoparty::VatNetwork::new(
                    futures::io::BufReader::new(reader),
                    futures::io::BufWriter::new(writer),
                    rpc_twoparty_capnp::Side::Server,
                    Default::default(),
                );

                let rpc_system = RpcSystem::new(Box::new(network), Some(client.clone().client));

                tokio::task::spawn_local(rpc_system);
            }
        });

        rt.block_on(local);
    });

    path
}
