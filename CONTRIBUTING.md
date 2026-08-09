# Contributing to miniperf

Run the workspace quality gates before submitting a change:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

## Profiler truth policy

Every new collector or analysis milestone in plans 02–12 must land with its
truth fixture. Each fixture must document its analytic answer, tolerance,
guarded plan milestone, required privileges, and unsupported platforms. A pure
test must exercise its assertion independently of hardware access; when useful,
include mutation-style evidence that a representative collector error fails.

Hardware-backed tests belong in the `truth` crate as ignored, privilege-aware
integration tests. Run them on controlled hardware using the instructions in
`truth/README.md`; GitHub-hosted runners do not provide a reliable hardware PMU.
Do not weaken or silently skip an assertion after recording has started.
