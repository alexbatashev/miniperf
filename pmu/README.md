# miniperf-pmu

The package is published as `miniperf-pmu`; its Rust library name remains
`pmu`. It exposes Linux perf counting and sampling, including `EventTimer` for
low-overhead measurements of a hot block:

```rust,no_run
use pmu::{Counter, EventTimer};

# fn work() {}
# fn example() -> Result<(), pmu::Error> {
let timer = EventTimer::new(&[Counter::Cycles, Counter::Instructions])?;
let span = timer.start()?;
work();
let measured = span.stop()?;
println!("{} cycles; IPC {:.2}", measured[Counter::Cycles], measured.ipc());
# Ok(())
# }
```

An `EventTimer` is bound to its creating thread and cannot be sent or shared.
Create one timer per worker using `EventTimer::new_for_thread`. All requested
events form a perf group, so they are scheduled together. Values returned by
indexing are scaled by `time_enabled / time_running`; `Measurement::raw` and
`Measurement::scaling` expose the underlying count and factor. `wall_ns` comes
from perf's enabled time; on the fast path the kernel metadata's
`cap_user_time` conversion advances that clock directly from TSC.

On Linux x86_64, the timer uses the kernel's perf metadata page and its seqlock
protocol to read enabled counters with `rdpmc`. On Linux AArch64 with kernel
5.17 or newer, the same mmap protocol selects direct PMUv3 EL0 register reads
when the administrator has enabled:

```console
sudo sysctl kernel.perf_user_access=1
```

The AArch64 path requests the kernel's per-event userspace-read flag, uses the
published mmap index to select an event register or `PMCCNTR_EL0`, honors the
published counter width and offset, and advances perf time from `CNTVCT_EL0`.
If mmap, `cap_user_rdpmc`, or the host policy does not permit direct access, both
architectures retain the same one-call grouped `read(2)` fallback.
`EventTimer::read_cost` reports the selected path (`Rdpmc`, `UserPmu`, or
`ReadSyscall`) and the median of 31 snapshots measured during construction.
This is more useful than a fixed overhead claim: cost varies with event count,
CPU, kernel, and virtualization. An empty span includes two snapshots and should
therefore be interpreted relative to roughly twice this reported snapshot cost.

## Quick in-memory sampling

`QuickSampler` profiles a closure and returns samples directly, without the
`mperf` dispatcher, a results directory, files, or an async runtime:

```rust,no_run
use pmu::{Counter, QuickSampler};

# fn work() {}
# fn example() -> Result<(), pmu::Error> {
let sampler = QuickSampler::new(&[Counter::Cycles])?;
let samples = sampler.record(4_000, work)?;
println!("collected {} samples", samples.len());
# Ok(())
# }
```

For long or untrusted-duration work, `QuickSampler::bounded` keeps only the
newest configured number of samples. `record_batch` returns a `SampleBatch`
whose `dropped_samples` value reports overwritten entries. `top_symbols`
performs best-effort lookup in the current process and falls back to hexadecimal
instruction pointers. With `symbolize` enabled, `SampleBatch::to_folded`
uses the shared resolver to expand inline frames and honor `.gnu_debuglink`,
build-ID cache entries, and `/tmp/perf-<pid>.map`, without creating a results
directory.

Enable the `symbolize` feature for `top_symbols` and the `quick_sample` example:

```console
cargo run -p miniperf-pmu --features symbolize --example quick_sample
```

## Features and compatibility

The default `events-native` feature embeds only event data for the compilation
target. `events-x86-64`, `events-aarch64`, and `events-riscv64` allow explicit
event-table selection with `default-features = false`. `symbolize` enables the
shared debug-aware in-process resolver, while `criterion` enables
`CriterionCounter`, a `criterion::measurement::Measurement` implementation:

```console
cargo run -p miniperf-pmu --features criterion --example criterion_counter
```

The crate follows Semantic Versioning. Incompatible public API changes require
a major release after 1.0 and a minor release before 1.0. The detailed policy
and release history live in [CHANGELOG.md](CHANGELOG.md).
