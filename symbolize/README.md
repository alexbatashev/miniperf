# miniperf-symbolize

Shared native symbolization for `mperf` and the optional `miniperf-pmu`
`symbolize` feature.

Resolution order on Linux is:

1. `/tmp/perf-<pid>.map` for JIT-generated code.
2. A valid `.gnu_debuglink` file beside the object, in `.debug/`, or beneath
   `/usr/lib/debug`.
3. The miniperf build-id cache at
   `~/.cache/miniperf/buildid/<hex-build-id>/debuginfo`.
4. The system build-id tree at `/usr/lib/debug/.build-id`.
5. The mapped object itself.

`MINIPERF_CACHE_DIR` overrides the cache root. `XDG_CACHE_HOME` is honored when
the miniperf-specific override is absent.

Network lookup is off by default. When both `MINIPERF_DEBUGINFOD=1` and
`DEBUGINFOD_URLS` are set, the resolver invokes an installed
`debuginfod-find debuginfo <build-id>` client and copies a successful result
into the miniperf cache. The crate contains no HTTP client, does not access the
network during builds or tests, and degrades to local symbols when the command
is unavailable or a server cannot be reached.
