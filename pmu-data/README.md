# miniperf-pmu-data

Serializable data types shared by miniperf's PMU event-table generator and
import tools. The Rust library name is `pmu_data`.

The schema covers processor-family metadata, raw events, portable aliases, and
derived metric expressions. JSON event encodings use hexadecimal strings such
as `"0x3c"` so imported vendor data remains easy to audit.

This crate is intentionally small: its only runtime dependency is Serde.

## Compatibility

The crate follows Semantic Versioning. After 1.0, incompatible schema or public
API changes require a major version. Before 1.0, they increment the minor
version. Additive optional fields use Serde defaults when backward-compatible.
