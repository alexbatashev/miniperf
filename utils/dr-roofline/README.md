# dr-roofline — DynamoRIO backend for roofline/memory accounting

DynamoRIO client emitting the same three artifacts as the QEMU plugin
(`qemu-roofline.counts`, `.cfg` dynamic-CFG v4, `.memory.json`); all analysis
lives in the shared `miniperf-roofline-core` crate (`utils/roofline-core`),
linked in as a staticlib via the C API in `roofline_core.h`.

DynamoRIO instruments natively, so it only works when the target matches the
host architecture — QEMU remains the cross-architecture path. `mperf` prefers
DynamoRIO automatically when `drrun` and this client are available, and if an
auto-selected DynamoRIO run fails at runtime it retries with the QEMU backend
instead of aborting (`--roofline-backend dynamorio|qemu` overrides and never
falls back; `--dynamorio`, `--dynamorio-client`, `MPERF_DYNAMORIO`,
`MPERF_DR_CLIENT` control discovery).

## Build

DynamoRIO must be built from master (releases lack the riscv64 port);
`utils/build-dynamorio-bundle.sh` does everything at a pinned revision.
Manually:

```sh
cargo build --release -p miniperf-roofline-core
cmake -B build -DDynamoRIO_DIR=<dynamorio-build>/cmake .
cmake --build build
<dynamorio-build>/bin64/drrun -disable_traces -max_bb_instrs 32 \
  -c build/libdr_roofline.so output=out.counts memory-profile=on -- ./app
```

When running mperf from a source checkout, the built client is discovered in
any CMake build directory directly below `utils/dr-roofline`; it does not need
to be copied beside the mperf executable. `--dynamorio` accepts either the
launcher or the DynamoRIO build directory, so no `PATH` or `LD_LIBRARY_PATH`
changes are normally needed:

```sh
mperf record --dynamorio <dynamorio-build> --scenario roofline \
  -o results -- ./app
```

Packaged bundles are also self-discovering. `--dynamorio-client` is only
needed when keeping a manually built client outside the source checkout or
bundle.

`-disable_traces -max_bb_instrs 32` are required: the per-instruction
instrumentation can exceed DynamoRIO's block emit limits at default sizes
(observed on riscv64). The mperf driver passes them automatically.

## Instrumentation design

Events are not delivered via clean calls. Each block execution and each memory
operand inline-writes a 16-byte `rc_record_t` into a per-thread 1MiB drx_buf
trace buffer; when the buffer fills (or the thread exits), one call into
`rc_process_batch` processes 64K records under a single session-mutex
acquisition. Translation-time metadata (block costs, operand sizes) is
registered once via `rc_register_block`/`rc_register_mem` and referenced from
records by handle. Batch processing run-length-aggregates back-to-back
self-loop block executions and skips CFG traffic attribution for accesses
whose modeled LLC traffic is zero. Per-thread event order is exact; with
multiple threads, interleaving across threads is approximated at buffer-flush
granularity (single-threaded results are bit-identical to per-event
processing — see `batch_processing_matches_per_event_processing` in
roofline-core). RVV operations still use clean calls because they must read
vl/vtype/vstart/v0 from the machine context at execution time.

## Validation status (Aug 2026, DR master 1fd3603b)

- x86_64: counts parity with the QEMU plugin (FP ops identical, bytes and
  modeled DRAM within 0.01%, instructions within 0.1%). Overhead vs native
  after the buffered-instrumentation redesign, measured on an optimized AVX
  matmul (worst case: ~4 records per 4-wide vector iteration): ~55×
  accounting-only (was ~680× with per-event clean calls). memory-profile=on
  remains dominated by the exact reuse-distance/working-set analysis
  (~2500× on the same matmul).
- x86_64 artifact equivalence: with ASLR disabled, the counters, per-block CFG
  and memory profile from the buffered client are byte-identical to the
  per-event clean-call client (modeled DRAM bytes vary by ±0.01% between runs
  under ASLR because set mapping depends on absolute addresses). The `image`
  line legitimately differs — see below.

Dynamic CFG v4 adds per-block `arch_bytes_load`/`arch_bytes_store` next to the
existing modeled-DRAM `bytes_load`/`bytes_store`.

Arithmetic intensity is computed from **architectural** traffic — the bytes the
block's load/store operands move — which is the cache-aware roofline (CARM)
convention and matches what the compiler-instrumentation backend already
reports. The original Williams/Waterman/Patterson roofline instead divides by
DRAM traffic; that is a different model, and it cannot be used here because a
loop whose working set fits in the LLC has zero modeled DRAM traffic, making
its intensity infinite and silently removing exactly the compute-bound loops
the chart exists to show. The two must not be mixed per loop: that would put
different loops on different x-axes and make a single loop's intensity jump
when its working set crosses the LLC.

The `roofline` view exposes `arch_bytes` and `dram_bytes` separately, so
`arch_bytes / dram_bytes` gives each loop's cache reuse (1x for a streaming
loop, unbounded for a cache-resident one). Unlike modeled DRAM stores,
architectural bytes are attributed to the issuing block for every access, so
their per-block sum must equal the whole-process totals exactly —
`validate_cfg_totals` enforces that.

Because intensity is architectural, a cache-resident loop legitimately sits
above the DRAM roof, so calibration measures a bandwidth roof per hierarchy
level (`memory_levels` in `info.json`) and the chart draws one line per level.
The triad used for those roofs is easy to get wrong: it needs an explicit
`target_feature` vector width (the crate builds for the x86-64 baseline), must
avoid `f64::mul_add` (a libm call without `+fma`), and must zip rather than
index its slices. With any of those wrong the loop is issue-bound and every
level reports the same figure. Levels whose measured bandwidth does not fall as
the working set grows are dropped rather than plotted.

The `image` start address reported to roofline-core is the runtime start of
the executable text segment, matching the QEMU plugin's
`qemu_plugin_start_code()`. mperf derives each loop's module offset as
`image_start - executable ELF segment link address`, so reporting
DynamoRIO's `main_module->start` (the module base, one page below text for a
PIE) skewed every module offset by the text segment's link address and
symbolized every loop to an unrelated symbol such as `__abi_tag`.
- riscv64 (Banana Pi F3, rv64gcv): RVV ops counted exactly (vl/vtype/vstart/v0
  from the mcontext, SEW-bucketed, masked ops supported); saxpy 20M/800M on
  the nose, zero rvv_state_errors.
- aarch64 (Orion O6): NEON classification exact (matmul 268,435,456 double
  ops); DynamoRIO's `$0x0N` element-size operand style is handled in
  roofline-core.

Known gaps: TMDL spec lacks the riscv bitmanip (Zba/Zbb) extension, so those
count as unclassified on hardware that compiles with them (same gap as the
QEMU path). riscv64/aarch64 have not been revalidated since the
buffered-instrumentation redesign (drx_buf is portable, but the boards should
be rerun). memory-profile=on overhead is dominated by the exact per-line
reuse-distance treap in roofline-core — sampling is the known next
optimization there.

Debug switches: `MPERF_DR_DEBUG_UNCLASSIFIED=1` prints unclassified
instructions at translation time; `MPERF_DR_DEBUG_CLASSIFY=1` prints every
classification.
