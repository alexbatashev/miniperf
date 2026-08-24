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

`deps/manifest.toml`'s `[support]` table lists the platforms each dependency
must cover. It is the only place that list exists: the workflow generates its
build matrix from it and refuses to publish a release — or rewrite the
manifest — that does not cover it, and CI re-checks the pinned manifest on
every pull request. Adding a platform means adding it there.

### How the pins are consumed

`store/build.rs` downloads the pinned DuckDB, verifies its checksum and unpacks
it into `deps/cache/duckdb`, which `.cargo/config.toml` points `libduckdb-sys`
at through `DUCKDB_LIB_DIR`. The first build on a machine therefore needs
network access; afterwards a stamp file makes it a no-op. Nothing compiles
DuckDB from source any more.

`utils/package-miniperf.sh` embeds the pinned DynamoRIO and qemu-user bundles
under `lib/miniperf`, where `mperf` finds them relative to its own executable
(`mperf/src/roofline/mod.rs`, `package_path`). `utils/verify-miniperf-package.sh`
fails the build if any of that is missing, so a package that would break on a
user's machine never gets uploaded.
