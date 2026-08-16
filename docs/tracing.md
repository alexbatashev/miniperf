# Tracing user code and runtimes

miniperf records more than PMU counters: any process can emit trace events
(spans, instants, counters) into the session directory through the collector
core (`libmperf_collector.so`). Events written by different processes, ranks,
and passes merge by concatenation; identity is a stable XXH3-64 hash of the
trace point's payload, so the same code location gets the same ID everywhere.

## C / C++ applications

Include `collector-core/include/mperf_trace.h` and link the static stub
(`mperf_trace_stub.c`); every call is a no-op unless the process runs under
`mperf record`:

```c
#include <mperf_trace.h>

void step(void) {
    MPERF_SCOPE("solver_step");        // C++ RAII span
    MPERF_COUNTER("residual", value);  // counter series
    MPERF_INSTANT("checkpoint", 0);    // instant marker
}
```

The full API (`mperf_trace_register` / `begin` / `end` / `instant` /
`counter`) is available for callsite-cached handles, explicit parenting, and
cross-thread spans. Register a payload with `MPERF_TRACE_FLAG_STACK` to opt
its callsite into frame-pointer stack capture.

## Rust applications

The `mperf-trace` crate wraps the same ABI:

```rust
let _guard = mperf_trace::trace_scope!("phase");
```

## Runtimes (no code changes)

Thin proxies forward runtime events through each API's native tool mechanism;
`mperf record` sets them up automatically:

| Runtime | Mechanism | Library |
|---------|-----------|---------|
| libc (allocations, mmap, threads) | `LD_PRELOAD` | `libmperf_libc.so` |
| OpenMP (parallel regions, tasks, barriers) | `OMP_TOOL_LIBRARIES` | `libmperf_ompt.so` |
| TBB / ITT (tasks, frames, domains) | `INTEL_LIBITTNOTIFY64` | `libmperf_itt.so` |
| CUDA (kernels, transfers) | `CUDA_INJECTION64_PATH` | `libmperf_cupti.so` |
| MPI (rank identity, clock exchange) | PMPI preload | `libmperf_mpi.so` |

Every shim is a pure-Rust cdylib crate under `shims/` with `extern "C"`
entry points; nothing in the workspace build needs a C toolchain, and
runtime ABIs (MPI flavor, libcupti) are resolved with dlopen/dlsym so the
shims cross-compile like any other crate. The only C source in the tree is
`collector-core/stub/mperf_trace_stub.c` + header, shipped for users to
compile into their own applications with their own toolchain.

The libc shim throttles allocator events (`MPERF_LIBC_SAMPLE_EVERY`, default
16; everything at or above `MPERF_LIBC_SIZE_THRESHOLD`, default 65536, is
always captured) and records the effective rates in the trace.

## Environment

- `MPERF_SESSION_DIR` — session directory; unset means tracing is off.
- `MPERF_COLLECTOR_LIBRARY` — explicit path to `libmperf_collector.so`.
- `MPERF_CONTROL_SHMEM` — optional control-channel prefix for live stats,
  pause/resume, and flush from a local miniperf. Remote ranks run file-only.

Loss policy: buffers are bounded; on exhaustion events are dropped and
counted, never blocking the application. Drops surface as `Loss` rows in the
events table and in control-plane stats.

## Visualization manifest

Presentation is declarative and attached in the viewer, never at record
time: drop a `manifest.yaml` (or `.json`) into the session directory and the
GUI applies it. The schema (`mperf_data::VisualizationManifest`, version 1)
covers track/lane assignment, marker glyphs and severity, counter
presentation (unit/scale/render/thresholds), and named custom views. Unknown
keys are ignored, so the vocabulary can grow without breaking. Without a
manifest, custom events surface as the `custom_events` table (grouped by
trace point, with span totals and counter sums) queryable via `mperf query`
and rendered as a table in the hotspots view.
