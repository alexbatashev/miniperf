# Memory profiling benchmarks

These deterministic single-process kernels exercise distinct memory behaviors:

- `matmul`: blocked-by-row dense matrix multiplication with high arithmetic
  intensity and strong reuse of the right-hand matrix.
- `stream`: sequential triad traffic with full cache-line utilization.
- `pointer-chase`: a randomized, single-cycle linked list with dependent loads,
  poor spatial locality, and a large reuse distance.
- `stencil`: a five-point grid update with neighborhood reuse.

The default STREAM arrays total 384 MiB, pointer chase uses a 128 MiB list,
and the two stencil grids total 256 MiB. These footprints exceed the shared LLC
on the development host. Matmul intentionally reuses a 6 MiB matrix set to
represent the compute-bound side of the comparison.

Build them from the repository root:

```sh
make -C examples/memory-benchmarks
```

Executables are written to `target/memory-benchmarks/bin/`. Each executable
accepts an optional problem size and repeat count. The defaults target roughly
15 seconds of native execution on the development machine, which is long enough
for stable wall-clock rates and high-confidence Roofline sampling. Pass smaller
repeat counts explicitly when iterating on exact memory analysis under QEMU.

Build native compiler-backend Roofline variants after building miniperf's
collector library:

```sh
cargo build -p collector
make -C examples/memory-benchmarks roofline
```

These variants submit explicit algorithmic operation and byte counts for the
timed top-level kernel. This allows the full 15-second workload to be measured
twice natively; the counts describe source-level work rather than modeled DRAM
traffic.

The checked-in sources are benchmarks, while generated recordings belong under
`target/memory-benchmarks/results/` and are intentionally not versioned.
