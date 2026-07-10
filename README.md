# miniperf

miniperf is a sampling profiler that provides an easy performance analysis
workflow for native applications across multiple architectures including X86,
AArch64, and RISC-V. It uses the same underlying APIs as Linux perf but
implements workarounds for platforms like SpacemiT X60 and offers a simpler
workflow.

## Features

- Simple, user-friendly interface for performance analysis
- Cross-platform support (X86, AArch64, RISC-V)
- Hardware counter sampling through `perf_event` APIs
- Basic performance statistics similar to perf stat
- Advanced sampling-based profiling with different analysis scenarios
- Workarounds for specific platform limitations
- Minimal dependencies and easy installation

## Installation

### Building from source

miniperf is implemented in Rust.

#### Requirements

1. Rust Toolchain
   1. Install Rust by following instructions on [rustup.rs](https://rustup.rs)
2. Clang 19 or 20 for Roofline analysis

#### Building

```sh
git clone https://github.com/alexbatashev/miniperf.git
cd miniperf
cargo build --release
```

#### Building Clang plugins

Roofline analysis requires compiler-based instrumentation. You need to build
a Clang plugin to make it work:

```sh
mkdir target/clang_plugin && cd target/clang_plugin
cmake -DCMAKE_BUILD_TYPE=Release -GNinja -DLLVM_DIR=$HOME/llvm-project/build/lib/cmake/llvm ../../utils/clang_plugin/
```

## Usage

### Basic Performance Statistics

Collect basic performance counter statistics similar to `perf stat`:

```sh
$ mperf stat -- /bin/ls -lah

<ls output>

Performance counter stats for '/bin/ls -lah':

+-------------------------+-----------+-----------------+---------+-----------------------------------------------------------+
| Counter                 | Value     | Info            | Scaling | Description                                               |
+=============================================================================================================================+
| cycles                  | 2,631,817 |                 |    1.00 | Number of CPU cycles                                      |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| instructions            | 2,409,166 | 0.92 inst/cycle |    1.00 | Number of instructions retired                            |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| llc_references          |   229,203 |                 |    2.01 | Last level cache references                               |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| llc_misses              |    43,718 | 18.15 MPKI      |    1.75 | Last level cache misses                                   |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| branch_misses           |    26,094 | 10.83 MPKI      |    1.60 | Branch instruction missess                                |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| branches                |   506,046 | 0.19 inst/cycle |    1.99 | Branch instructions retired                               |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| stalled_cycles_backend  |   393,366 | 14.95%          |    2.34 | Number of cycles stalled due to backend bottlenecks       |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| stalled_cycles_frontend |   193,237 | 7.34%           |    2.66 | Number of cycles stalled due to frontend bottlenecks      |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| cpu_clock               | 5,957,570 |                 |    1.00 | A high-resolution per-CPU timer                           |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| cpu_migrations          |         0 |                 |    1.00 | Number of the times the process has migrated to a new CPU |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| page_faults             |       162 |                 |    1.00 | Number of page faults                                     |
|-------------------------+-----------+-----------------+---------+-----------------------------------------------------------|
| context_switches        |         0 |                 |    1.00 | Number of context switches                                |
+-------------------------+-----------+-----------------+---------+-----------------------------------------------------------+
```

Use `mperf list` to discover model-specific PMU events and select one or more
with `-e`:

```sh
mperf stat -e L1D.REPLACEMENT,BR_MISP_RETIRED.ALL_BRANCHES -- ./workload
```

### Recording Profiles

Record detailed performance profiles for in-depth analysis:

```sh
mperf record -s <scenario_name> -o <output_directory> -- <your_command_and_arguments>
```

Available Scenarios

- `snapshot`: A basic performance snapshot similar to stat command but in
  sampling mode. Useful for general performance overview.
- `roofline`: Roofline analysis capture that requires instrumented binaries.
  This runs collection in two passes:
    1. First to collect PMU (Performance Monitoring Unit) counters
    2. Second to gather loop statistics

#### Call-stack collection overhead

On x86-64, `mperf record` first requests Intel Last Branch Record call stacks.
LBR collection adds only the hardware branch entries to each sample and avoids
copying the user stack. Opening the perf event is also the runtime capability
probe: AMD systems, VMs, and Intel PMUs without call-stack LBR support
automatically retry in DWARF mode.

DWARF mode captures the user registers and up to 8 KiB of stack, then unwinds
that data after the target exits; the raw state is stored once and reused by all
counters in the group. This produces useful stacks for optimized binaries that
omit frame pointers, at the cost of up to 8 KiB of ring-buffer traffic and
result data per interrupt (the kernel reports the bytes it could actually copy).

Library users can trade stack depth and recording overhead with
`SamplingDriverBuilder::stack_dump_size`, or select
`UnwindMode::FramePointer` to disable register/stack capture and retain the
kernel callchain path. `UnwindMode::Lbr` explicitly requests the LBR-first mode
with the same automatic DWARF fallback.

#### Symbols and separate debug information

Postprocessing expands DWARF inline frames and uses the shared
`miniperf-symbolize` library for symbols and source locations. It understands
`.gnu_debuglink`, system and miniperf build-id caches, and
`/tmp/perf-<pid>.map` JIT symbol files. See [`symbolize/README.md`](symbolize/README.md)
for lookup order, cache paths, and the explicitly opt-in debuginfod behavior.

#### Building instrumented application

Roofline analysis requires instrumented binaries to work properly. Here's how
you can use Clang plugin to build your application:

```sh
clang -O3 source.c -o a.out -g -Xclang -fpass-plugin=$HOME/miniperf/target/clang_plugin/lib/miniperf_plugin.so -L $HOME/miniperf/target/release/ -lcollector
```

### Viewing Results

After recording a profile, you can view the results with:

```sh
mperf show <output_directory>
```

This will display detailed analysis based on the recorded profile.

## Platform-Specific Notes

### Intel Tiger Lake

- Models 0x8c and 0x8d are detected as Tiger Lake.
- The checked-in table contains 231 core events generated from Linux perf's
  Tiger Lake PMU data. See `pmu/events/intel/README.md` for the source,
  attribution, licensing, and regeneration command.
- Unsupported architectural counters are omitted with a notice instead of
  aborting the entire `stat` or sampling run.

### AArch64 (Arm)

- CPU cores are identified from `MIDR_EL1` (implementer + part number). Cortex-A720
  and Cortex-A520 are shipped with curated PMU event sets; other cores fall back
  to the architectural events exposed by `perf_event`.
- On heterogeneous (big.LITTLE) systems each cluster exposes its own PMU with a
  distinct `perf_event` type. `mperf stat` opens every hardware counter on *each*
  cluster's PMU, so a task is counted correctly wherever it runs. Results are
  reported per core cluster plus a faithful total summed across clusters. Per-core
  values are raw on-cluster counts (never extrapolated across clusters).
- `mperf record` likewise samples on every cluster's PMU, so execution on any core
  is captured. In addition to the merged `flamegraph_cycles.{svg,folded}`, per-core
  flamegraphs are written as `flamegraph_cycles_<family>.{svg,folded}` (and the same
  for instructions), e.g. `flamegraph_cycles_cortex_a720.svg`.
- By default the detected primary (first recognized) core determines the CPU
  family used for event names and sampling. To target a specific cluster
  explicitly — for example to profile the little cluster — set
  `MINIPERF_CPU_FAMILY` and pin the workload to that cluster:

  ```sh
  MINIPERF_CPU_FAMILY=cortex_a520 taskset -c 1-4 mperf stat -- ./workload
  ```

### SpacemiT X60

- SpacemiT X60 cores do not implement overflow interrupt for cycles or
  instructions counters. Sampling is performed on `u_mode_cycles` event for all
  collections, sampling on M mode instructions is unavailable.
- Cache references and cache missess are mapped to `l2_access` and `l2_miss`
  events respectively.
