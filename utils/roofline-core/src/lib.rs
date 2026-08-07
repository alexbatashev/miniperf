//! Shared roofline/memory-analysis core used by both instrumentation
//! backends: the QEMU TCG plugin (`utils/qemu-roofline`) and the DynamoRIO
//! client (`utils/dr-roofline`). The artifact file formats written here are
//! the contract consumed by `mperf` postprocessing.

pub mod artifacts;
pub mod cache;
pub mod capi;
pub mod cfg;
pub mod classify;
pub mod memory;

pub use artifacts::{CacheDescription, CounterSnapshot, ImageInfo};
pub use cache::{CacheModel, MemoryTraffic};
pub use cfg::{DynamicBlockCounts, DynamicCfg, VectorClass};
pub use classify::{
    active_elements, classify_aarch64, classify_flow, classify_riscv, classify_x86, is_masked,
    mnemonic, rvv_sew, BlockCost, FlowKind, RiscvClassification, RiscvCost, RiscvKind, RvvKind,
    Target,
};
pub use memory::{MemoryAnalysis, MemoryArtifact, WorkingSetArtifact, WORKING_SET_WINDOWS};
