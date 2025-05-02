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

miniperf is implemented in Rust and requires Cap'n Proto as a dependency.

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
````

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

### SpacemiT X60

- SpacemiT X60 cores do not implement overflow interrupt for cycles or
  instructions counters. Sampling is performed on `u_mode_cycles` event for all
  collections, sampling on M mode instructions is unavailable.
- Cache references and cache missess are mapped to `l2_access` and `l2_miss`
  events respectively.
