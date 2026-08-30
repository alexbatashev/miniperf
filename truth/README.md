# miniperf truth suite

This crate contains controlled native fixtures and assertions with analytically
known answers. Its test failures name the roadmap milestone they guard.

`build.rs` compiles every fixture at `-O2 -g` both with and without frame
pointers. `duty_split` is active for 01-F6.1. `known_sleeper` is present now,
but its `250 ms ±5%` database assertion remains gated on off-CPU profiling in
08-M1.

The pure assertions, including mutation evidence, run normally:

```sh
cargo test -p truth
```

The profiler integration tests run under plain `cargo test` and fail on a host
without perf access. A kernel that exposes no hardware PMU at all (a VM) skips
them by probe; hosts that cannot profile for policy reasons must say so
explicitly:

```sh
sudo sysctl -w kernel.perf_event_paranoid=-1   # to run them
MPERF_NO_PMU=1 cargo test -p truth             # to skip them, visibly
```

GitHub-hosted CI sets `MPERF_NO_PMU=1`; tests never change host policy
themselves. `MPERF_BIN` may override the binary path.

## Fixture policy

Every collector or analysis milestone in plans 02–12 must add or activate a
truth fixture in the same change. A fixture must state its analytic answer,
tolerance, guarded milestone, required privileges, and unsupported platforms.
The privileged truth job is the merge bar for Linux collectors.
