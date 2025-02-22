use std::{process, sync::Arc, thread};

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use futures::{AsyncReadExt, StreamExt};
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

// pub fn create_ipc_server<F: Fn(IPCMessage) + Send + 'static>(callback: F) -> String {
//     let path = format!("/tmp/{}.sock", process::id());
//
//     let boxed_callback = Box::new(callback);
//
//     let task_path = path.clone();
//     thread::spawn(move || {
//         let rt = tokio::runtime::Builder::new_current_thread()
//             .enable_io()
//             .build()
//             .unwrap();
//         let local = LocalSet::new();
//         local.spawn_local(async move {
//             let listener = UnixListener::bind(task_path).expect("Could not create unix socket");
//             let client: mperf_data::IPCClient =
//                 capnp_rpc::new_client(IPCServer::new(boxed_callback));
//
//             loop {
//                 let (stream, _) = listener.accept().await.expect("failed to accept a stream");
//                 let (reader, writer) = TokioAsyncReadCompatExt::compat(stream).split();
//
//                 let network = twoparty::VatNetwork::new(
//                     futures::io::BufReader::new(reader),
//                     futures::io::BufWriter::new(writer),
//                     rpc_twoparty_capnp::Side::Server,
//                     Default::default(),
//                 );
//
//                 let rpc_system = RpcSystem::new(Box::new(network), Some(client.clone().client));
//
//                 println!("ACCEPTED A CONNECTION!!!!");
//                 // tokio::task::spawn_local(rpc_system);
//                 tokio::spawn(async move {
//                     if let Err(e) = rpc_system.await {
//                         eprintln!("RPC error: {:?}", e);
//                     }
//                 });
//             }
//         });
//
//         println!("WILL BLOCK!!!");
//         rt.block_on(local);
//         println!("THREAD FINISHED!!!");
//     });
//
//     path
// }

pub fn create_ipc_server<F: Fn(IPCMessage) + Send + Clone + 'static>(callback: F) -> String {
    let path = format!("/tmp/{}.sock", process::id());
    let task_path = path.clone();
    let boxed_callback = Box::new(callback);

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_io()
            .build()
            .unwrap();

        rt.block_on(async move {
            let listener = UnixListener::bind(&task_path).expect("Could not create unix socket");
            let client: mperf_data::IPCClient =
                capnp_rpc::new_client(IPCServer::new(boxed_callback));

            let mut active_connections = futures::stream::FuturesUnordered::new();

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        let (stream, _) = accept_result.expect("failed to accept a stream");
                        let (reader, writer) = TokioAsyncReadCompatExt::compat(stream).split();

                        let network = twoparty::VatNetwork::new(
                            futures::io::BufReader::new(reader),
                            futures::io::BufWriter::new(writer),
                            rpc_twoparty_capnp::Side::Server,
                            Default::default(),
                        );

                        let rpc_system = RpcSystem::new(Box::new(network), Some(client.clone().client));
                        println!("ACCEPTED A CONNECTION!!!!");

                        active_connections.push(rpc_system);
                    }
                    Some(connection_result) = active_connections.next(), if !active_connections.is_empty() => {
                        println!("BOOOM!");
                        if let Err(e) = connection_result {
                            eprintln!("RPC error: {:?}", e);
                        }
                    }
                }
            }
        });
    });

    path
}
