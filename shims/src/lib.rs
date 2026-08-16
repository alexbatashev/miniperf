//! Locator for the miniperf proxy shims (§4 of the event-collection
//! redesign). Each shim is a pure-Rust cdylib crate under `shims/`, built by
//! cargo like every other workspace member — no external C toolchain.

use std::path::PathBuf;

fn next_to_current_exe(name: &str) -> Option<PathBuf> {
    let mut path = std::env::current_exe().ok()?;
    path.pop();
    for candidate in [path.join(name), path.join("../lib").join(name)] {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Path of the libc LD_PRELOAD shim, when built for this target.
pub fn libc_shim() -> Option<PathBuf> {
    next_to_current_exe("libmperf_libc.so")
}

/// Path of the OMPT tool library (OMP_TOOL_LIBRARIES).
pub fn ompt_shim() -> Option<PathBuf> {
    next_to_current_exe("libmperf_ompt.so")
}

/// Path of the ITT collector (INTEL_LIBITTNOTIFY64).
pub fn itt_shim() -> Option<PathBuf> {
    next_to_current_exe("libmperf_itt.so")
}

/// Path of the MPI proxy (PMPI preload).
pub fn mpi_shim() -> Option<PathBuf> {
    next_to_current_exe("libmperf_mpi.so")
}

/// Path of the CUPTI injection library (CUDA_INJECTION64_PATH).
pub fn cupti_shim() -> Option<PathBuf> {
    next_to_current_exe("libmperf_cupti.so")
}
