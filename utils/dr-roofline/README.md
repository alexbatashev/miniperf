# dr-roofline — DynamoRIO backend for roofline/memory accounting

DynamoRIO client emitting the same three artifacts as the QEMU plugin
(`qemu-roofline.counts`, `.cfg` dynamic-CFG v3, `.memory.json`); all analysis
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

`-disable_traces -max_bb_instrs 32` are required: the per-instruction clean
calls exceed DynamoRIO's block emit limits at default sizes (observed on
riscv64). The mperf driver passes them automatically.

## Validation status (Aug 2026, DR master 1fd3603b)

- x86_64: counts parity with the QEMU plugin (FP ops identical, bytes and
  modeled DRAM within 0.01%, instructions within 0.1%). Overhead vs native:
  ~63× accounting-only, ~600× with memory-profile=on — versus QEMU's 247× /
  917× on the same workloads.
- riscv64 (Banana Pi F3, rv64gcv): RVV ops counted exactly (vl/vtype/vstart/v0
  from the mcontext, SEW-bucketed, masked ops supported); saxpy 20M/800M on
  the nose, zero rvv_state_errors.
- aarch64 (Orion O6): NEON classification exact (matmul 268,435,456 double
  ops); DynamoRIO's `$0x0N` element-size operand style is handled in
  roofline-core.

Known gaps: TMDL spec lacks the riscv bitmanip (Zba/Zbb) extension, so those
count as unclassified on hardware that compiles with them (same gap as the
QEMU path). Memory-profile overhead is dominated by per-access clean calls
into the shared cache model/treap — batching addresses in TLS buffers
(drmemtrace-style) is the known next optimization.

Debug switches: `MPERF_DR_DEBUG_UNCLASSIFIED=1` prints unclassified
instructions at translation time; `MPERF_DR_DEBUG_CLASSIFY=1` prints every
classification.
