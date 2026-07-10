# PMU event importer

`event-import` converts upstream vendor PMU descriptions to
`miniperf-pmu-data`'s `PlatformDesc` JSON. It is a workspace development tool
and is not shipped with miniperf.

## Usage

Import one direct [Intel perfmon] event JSON file:

```text
cargo run -p event-import -- intel <events.json> <output.json> <family-id> <name>
```

The `intel` mode also accepts a Linux perf family directory and deterministically
combines its core event and metric JSON files. `intel-linux` remains as a
compatibility alias for this directory-oriented usage. Uncore files and event
encodings requiring extra MSRs are excluded because `PlatformDesc` cannot yet
represent them.

Import the `events` object from an [Arm Telemetry Solution] CPU PMU file:

```text
cargo run -p event-import -- arm-telemetry <cpu.json> <output.json> <family-id> <name>
```

The Arm importer preserves each mnemonic, event code, and full description
(falling back to the title when no description is supplied). Events whose
explicit `accesses` list does not include `PMU` are excluded. Portable aliases
are emitted only when their architectural event is present. Counter counts are
left unspecified because they are CPU-specific and are not part of an event
definition.

## Source attribution and licensing

- Intel's event schema and data come from [intel/perfmon]. Consult that
  repository's `LICENSE` and per-file notices before checking in generated
  output.
- Linux perf PMU data comes from the Linux kernel's
  `tools/perf/pmu-events`; it is distributed under GPL-2.0-only. See the Linux
  repository's `COPYING` file and retain source-revision attribution next to
  generated tables.
- Arm event schema and data come from the [Arm Telemetry Solution], Copyright
  Arm Limited and contributors, distributed under Apache-2.0 in the upstream
  repository. Retain upstream notices and record the exact source revision for
  generated tables.

The small files under `tests/fixtures` are hand-written schema samples used to
exercise compatibility; they are not generated vendor event tables.

[Intel perfmon]: https://github.com/intel/perfmon
[Arm Telemetry Solution]: https://gitlab.arm.com/telemetry-solution/telemetry-solution
