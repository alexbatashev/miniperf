# Intel PMU event data

The generated event and metric tables in this directory are derived from Linux
perf's `tools/perf/pmu-events/arch/x86` data. Regenerate a table with:

```text
cargo run -p event-import -- intel-linux <linux-family-dir> \
  pmu/events/intel/<family>.json <family-id> <display-name>
```

Source for `tigerlake.json`:
https://github.com/torvalds/linux/tree/master/tools/perf/pmu-events/arch/x86/tigerlake

Linux perf's event data is distributed under GPL-2.0-only. The source repository
and its `COPYING` file are authoritative for licensing and attribution.

The importer reads core event JSON and perf-style `MetricName`/`MetricExpr`
definitions, skips uncore and extra-MSR encodings that the current schema cannot
represent, and emits deterministic name-sorted output. The checked-in Tiger Lake
table includes the representative `IPC = instructions / cycles` metric used by
the evaluator, stat, and persistence tests.
