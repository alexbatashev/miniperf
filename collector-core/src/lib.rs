//! Self-contained trace collector core: proxies and instrumented binaries
//! load this library and forward events through the C ABI; it writes its own
//! event/string/payload/clock Parquet tables into `$MPERF_SESSION_DIR`.

mod api;
mod buffer;
mod control;
mod legacy;
mod session;
mod stack;

pub use api::*;
pub use control::{CollectorStats, ControlClient, ControlCommand};
pub use legacy::*;
pub use session::{Collector, EventRecord, HandleData, TraceKind, collector, shutdown};

/// Environment variable naming the session directory. Unset means every
/// entry point is a no-op.
pub const SESSION_DIR_ENV: &str = "MPERF_SESSION_DIR";
