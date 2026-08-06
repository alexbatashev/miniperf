# Roofline analysis with miniperf

Roofline analysis relates a workload's computational throughput to the amount
of data it moves. It helps answer two practical questions:

1. Is this workload limited primarily by compute throughput or memory
   bandwidth?
2. How close is it to the relevant machine ceiling?

For arithmetic intensity \(I\) in operations per byte, the attainable
throughput is:

$$
P(I) = \min(P_\text{compute}, I \times B_\text{memory})
$$

Here, \(P_\text{compute}\) is measured in GFLOP/s and \(B_\text{memory}\) in
GB/s. Their intersection is the ridge point:

$$
I_\text{ridge} = P_\text{compute} / B_\text{memory}
$$

A point to the left of the ridge is normally bandwidth-bound. A point to the
right is normally compute-bound.

This comparison is valid only when the point's byte denominator and the
bandwidth ceiling describe the same memory-hierarchy level. The QEMU plugin
retains exact architectural load/store bytes for audit, and separately runs a
deterministic shared-LLC model configured from the host's sysfs cache geometry.
The modeled LLC misses and dirty-line transitions form the byte denominator
used with the DRAM-sized streaming ceiling. The viewer labels this explicitly
as modeled traffic: it is suitable for an Advisor-style model Roofline, but it
is not a claim that hardware memory-controller transactions were measured.

## Build miniperf

Build the profiler and QEMU accounting plugin:

```sh
cargo build --release -p mperf -p miniperf-qemu-roofline
```

The QEMU backend requires a user-mode QEMU executable whose `--help` output
lists `-plugin`. You can build the pinned QEMU version used by miniperf:

```sh
utils/build-qemu-user-bundle.sh dist
```

The script creates a versioned archive under `dist/`. Extract it and use the
appropriate executable from its `bin/` directory. Before packaging, the script
checks the required exported plugin APIs and runs the miniperf plugin end to end
against static x86-64 SSE2 and RISC-V RVV fixtures with exact expected operation
and modeled traffic counts. The build fails if either capture has incomplete
counters, RVV state errors, unclassified compute instructions, malformed cache
metadata, or malformed CFG block ranges.

## One command, automatic method selection

The user-facing workflow is always:

```sh
mperf record --scenario=roofline --output-directory results -- ./workload
```

Miniperf inspects the executable and probes QEMU plugin support before starting
the workload. It then selects, in order:

1. QEMU operation and shared-LLC traffic accounting plus native performance
   measurement when the executable can run on the host.
2. The compiler-instrumented method when instrumentation is present and QEMU is
   unavailable.

If none is accurate enough, recording stops with a capability diagnostic. The
`--roofline-backend` option remains available as a testing/debugging override,
but normal users should not need to choose a backend. In particular, automatic
mode refuses cross-architecture emulator timing because it cannot represent
RISC-V hardware performance; run the same command on a compatible RISC-V host.

## Build the SpMV example

The CRS SpMV example supplies explicit AVX2, AVX-512, and RVV kernels. Use the
AVX2 binary for QEMU TCG analysis; QEMU 11 TCG does not implement AVX-512:

```sh
make -C examples/spmv-crs
```

Its build directory and any `results/` directory below the example are ignored
by Git.

## Keep the calibration and workload comparable

Miniperf measures the FP64 compute ceiling and a DRAM-sized streaming-bandwidth
ceiling immediately before every Roofline recording. The calibration and
workload run in the same process affinity, but Rayon and OpenMP have independent
worker counts. The memory ceiling is retained for provenance, but is used in a
Roofline comparison only when the recording reports a compatible traffic level.

Set both explicitly when studying a fixed CPU set:

```sh
export RAYON_NUM_THREADS=4
export OMP_NUM_THREADS=4
export OMP_PLACES=cores
export OMP_PROC_BIND=close
```

Run `taskset` around `mperf`, not only around the workload. This constrains the
calibration, QEMU, and guest consistently:

```sh
taskset -c 0-3 target/release/mperf ...
```

Choose physical cores when possible. Avoid running unrelated CPU- or
memory-intensive work during calibration.

## Record an x86 workload through QEMU

The following command records the AVX2 SpMV example. Replace the QEMU path
with the plugin-enabled binary you installed or extracted:

```sh
taskset -c 0-3 target/release/mperf record \
  --scenario=roofline \
  --qemu /path/to/qemu-x86_64 \
  --output-directory examples/spmv-crs/results/avx2-4t \
  -- examples/spmv-crs/build/spmv-avx2
```

Miniperf performs three pieces of work:

1. It measures host FP64 and memory ceilings with Rayon.
2. It runs the workload natively for duration and PMU sampling.
3. It runs the workload again with the QEMU plugin for operation, exact
   architectural-byte, and modeled DRAM-traffic accounting.

The accounting run captures an observed translation-block CFG, summarizes
call/return pairs, and writes natural-loop candidates with latches, nesting,
trip counts, inclusive counts, self counts, stable module offsets, and source
locations (when debug information is available) to
`qemu-roofline.loops.json`. Native PMU samples are matched to those block ranges
after recording. A loop receives a plotted throughput point only when its
estimated 95% timing error is at most 10%; lower-confidence loops remain visible
with their accounting and confidence state. Loops from shared libraries and the
dynamic loader are excluded.

## Record an RVV workload through QEMU

For the Ubuntu RISC-V cross sysroot:

```sh
taskset -c 0-3 target/release/mperf record \
  --scenario=roofline \
  --qemu /path/to/qemu-riscv64 \
  --qemu-arg=-L \
  --qemu-arg=/usr/riscv64-linux-gnu \
  --qemu-arg=-cpu \
  --qemu-arg=rv64,v=true,vlen=256 \
  --output-directory examples/spmv-crs/results/rvv-vlen256 \
  -- examples/spmv-crs/build/spmv-rvv
```

RVV accounting uses the executed instruction's runtime `vl`, `vstart`, SEW,
and mask state. Changing `vlen` therefore changes lane capacity without
changing the accounting rule.

On a non-RISC-V host automatic mode stops because native RISC-V performance is
unavailable. Explicit `--roofline-backend qemu` remains an emulation diagnostic:
its operation counts describe the guest instruction stream, but its throughput
is based on host QEMU duration and is never a projected RISC-V hardware result.
On a compatible RISC-V host, the normal command uses native performance
measurement automatically and QEMU only for accounting.

## Use the compiler backend

The compiler backend provides source-loop rows instead of one process-wide
row. Build the collector and the LLVM pass first:

```sh
cargo build --release -p mperf -p collector
cmake -S utils/clang_plugin -B target/clang_plugin -GNinja \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_DIR=/path/to/llvm/lib/cmake/llvm
cmake --build target/clang_plugin
```

Compile the workload with the pass and collector:

```sh
clang -O3 -g source.c -o workload \
  -Xclang -fpass-plugin=target/clang_plugin/lib/miniperf_plugin.so \
  -L target/release -lcollector
```

Then record without selecting an alternate backend:

```sh
target/release/mperf record \
  --scenario roofline \
  --output-directory results/compiler \
  -- ./workload
```

## Open and inspect a recording

Open the interactive result viewer:

```sh
target/release/mperf show examples/spmv-crs/results/avx2-4t
```

Use `Tab` and `Shift-Tab` to move between Summary, Loops, and Flamegraph; press
`?` for help and `q` to exit.

The recording directory contains:

- `info.json`: command, backend, CPU description, and calibrated roofs.
- `perf.db`: processed PMU and Roofline tables and views.
- `events.bin`, `strings.json`, and `proc_map.json`: raw capture data.
- `flamegraph_*.svg`: generated flame graphs for sampled counters.
- `qemu-roofline.counts`: QEMU plugin totals when using that backend.
- `qemu-roofline.cfg`: observed QEMU translation-block entries, edges, and
  per-block counts.
- `qemu-roofline.loops.json`: binary loop candidates and per-loop accounting.

The Summary tab reports the calibrated FP64 and streaming-memory ceilings. The
full calibration is available in JSON:

```sh
jq '.cpu_info.roofline_calibration' \
  examples/spmv-crs/results/avx2-4t/info.json
```

It includes all five samples, their medians, the Rayon worker count, CPU
affinity, compute kernel, memory working-set size, and ridge point. Large sample
variation usually means the machine was busy, frequency behavior changed, or
the selected CPUs were not isolated.

## Read the Loops table

The Loops tab separates scalar and vector operations by single and double
precision. Each category has two values:

- `GFLOP/s` is the counted operations divided by measured duration.
- `AI` is counted operations divided by counted load and store bytes.

The calibrated compute roof is currently FP64, so compare it with the double
precision columns. If a loop mixes scalar and vector FP64, add their DP
throughputs before comparing with the aggregate FP64 roof.

The following calculation applies only to a recording whose byte denominator
is explicitly marked as measured or modeled DRAM traffic. QEMU rows use the
modeled form and the UI labels them accordingly. Suppose compatible calibration
reports:

```text
FP64 compute roof: 240 GFLOP/s
Memory roof:        32 GB/s
Ridge point:         7.5 FLOP/byte
```

At an arithmetic intensity of 0.8 FLOP/byte, the bandwidth ceiling is:

```text
0.8 FLOP/byte × 32 GB/s = 25.6 GFLOP/s
```

An observed 18 GFLOP/s is therefore about 70% of the relevant roof. At an
intensity of 12 FLOP/byte, the compute ceiling of 240 GFLOP/s applies; an
observed 150 GFLOP/s is about 63% of that roof.

You can query the same values directly:

```sh
sqlite3 examples/spmv-crs/results/avx2-4t/perf.db \
  'SELECT function_name, vector_double_ops / 1e9 AS vector_dp_gflops,
          vector_double_ai
     FROM roofline;'
```

## Interpret the result carefully

- For a recording with compatible hierarchy traffic, a point far below the
  sloped roof can indicate poor locality, insufficient memory-level parallelism,
  synchronization, or accounting that includes work outside the kernel.
- A point far below the horizontal roof can indicate dependency chains,
  instruction-mix limits, front-end pressure, or too little parallel work.
- QEMU's exact architectural guest load/store totals remain in
  `qemu-roofline.counts`. Roofline loop bytes come from a shared, set-associative
  LRU LLC model using the detected host line size, capacity, and associativity.
  It models write-back traffic without read-for-ownership and is not a hardware
  memory-controller measurement.
- The SpMV executable's printed byte model is algorithmic and may differ from
  both architectural traffic and DRAM traffic.
- The compiler backend attributes data to instrumented source loops. The QEMU
  backend discovers dynamic binary loops in the main executable and excludes
  the loader and shared-library address ranges.
- Compare recordings only when affinity, thread counts, matrix size, precision,
  backend, and calibration conditions are equivalent.

Repeat important measurements and retain the raw calibration samples in each
recording rather than copying a single machine-wide roof between experiments.
