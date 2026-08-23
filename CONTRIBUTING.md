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

## External binary dependencies

DuckDB, DynamoRIO and qemu-user are not built by the main CI gate. The
`Dependencies` workflow builds them for every platform that supports them,
publishes them as a dated prerelease, and opens a pull request repinning
`deps/manifest.toml`; it also runs monthly to pick up new DynamoRIO commits and
QEMU releases. DuckDB is excluded from that automatic refresh because
`miniperf-store` links it against the `duckdb` crate's pregenerated bindings —
bump the crate and the pin together.

To reproduce one dependency locally:

```sh
utils/build-duckdb-bundle.sh linux-x86_64 dist
utils/build-dynamorio-bundle.sh linux-x86_64 dist
utils/build-qemu-user-bundle.sh linux-x86_64 dist
```

`linux-riscv64` cross-compiles; run `utils/deps/setup-riscv64-sysroot.sh` first
on an amd64 Ubuntu host.
