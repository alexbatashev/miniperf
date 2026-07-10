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

The profiler integration test is ignored because hardware perf access is a
host policy decision. On Linux, build `mperf`, allow perf events, and run:

```sh
sudo sysctl -w kernel.perf_event_paranoid=-1
cargo build -p mperf
cargo test -p truth --test profile -- --ignored
```

The ignored test checks privileges before recording and reports a skip when
they are unavailable. CI is responsible for setting the sysctl; tests never
change host policy themselves. `MPERF_BIN` may override the binary path.

## Fixture policy

Every collector or analysis milestone in plans 02–12 must add or activate a
truth fixture in the same change. A fixture must state its analytic answer,
tolerance, guarded milestone, required privileges, and unsupported platforms.
The privileged truth job is the merge bar for Linux collectors.
