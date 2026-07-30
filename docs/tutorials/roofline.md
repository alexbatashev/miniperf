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
appropriate executable from its `bin/` directory.

## Build the SpMV example

The CRS SpMV example supplies explicit AVX-512 and RVV kernels:

```sh
make -C examples/spmv-crs
```

Its build directory and any `results/` directory below the example are ignored
by Git.

## Keep the calibration and workload comparable

Miniperf measures the FP64 compute roof and memory-bandwidth roof immediately
before every Roofline recording. The calibration and workload run in the same
process affinity, but Rayon and OpenMP have independent worker counts.

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

The following command records the AVX-512 SpMV example. Replace the QEMU path
with the plugin-enabled binary you installed or extracted:

```sh
taskset -c 0-3 target/release/mperf record \
  --scenario roofline \
  --roofline-backend qemu \
  --qemu /path/to/qemu-x86_64 \
  --output-directory examples/spmv-crs/results/avx512-4t \
  -- examples/spmv-crs/build/spmv-avx512
```

Miniperf performs three pieces of work:

1. It measures host FP64 and memory ceilings with Rayon.
2. It runs the workload under QEMU for duration and PMU sampling.
3. It runs the workload again with the QEMU plugin for operation and byte
   accounting.

The QEMU backend emits one whole-process Roofline row.

## Record an RVV workload through QEMU

For the Ubuntu RISC-V cross sysroot:

```sh
taskset -c 0-3 target/release/mperf record \
  --scenario roofline \
  --roofline-backend qemu \
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

This measures the guest while it is emulated. The operation counts and
arithmetic intensity describe the guest instruction stream, but throughput is
based on host QEMU duration. Do not treat it as projected RISC-V hardware
performance.

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
target/release/mperf show examples/spmv-crs/results/avx512-4t
```

Use `Tab` and `Shift-Tab` to move between Summary, Loops, and Flamegraph; press
`?` for help and `q` to exit.

The recording directory contains:

- `info.json`: command, backend, CPU description, and calibrated roofs.
- `perf.db`: processed PMU and Roofline tables and views.
- `events.bin`, `strings.json`, and `proc_map.json`: raw capture data.
- `flamegraph_*.svg`: generated flame graphs for sampled counters.
- `qemu-roofline.counts`: QEMU plugin totals when using that backend.

The Summary tab reports the calibrated FP64 and memory roofs. The full
calibration is available in JSON:

```sh
jq '.cpu_info.roofline_calibration' \
  examples/spmv-crs/results/avx512-4t/info.json
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

For example, suppose calibration reports:

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
sqlite3 examples/spmv-crs/results/avx512-4t/perf.db \
  'SELECT function_name, vector_double_ops / 1e9 AS vector_dp_gflops,
          vector_double_ai
     FROM roofline;'
```

## Interpret the result carefully

- A point far below the sloped roof can indicate poor locality, insufficient
  memory-level parallelism, synchronization, or accounting that includes work
  outside the kernel.
- A point far below the horizontal roof can indicate dependency chains,
  instruction-mix limits, front-end pressure, or too little parallel work.
- QEMU counts architectural guest loads and stores, not physical DRAM
  transactions. Cached accesses are included.
- The SpMV executable's printed byte model is algorithmic and may differ from
  both architectural traffic and DRAM traffic.
- The compiler backend attributes data to instrumented source loops; the QEMU
  backend currently attributes the whole guest process, including loader and
  library activity.
- Compare recordings only when affinity, thread counts, matrix size, precision,
  backend, and calibration conditions are equivalent.

Repeat important measurements and retain the raw calibration samples in each
recording rather than copying a single machine-wide roof between experiments.
