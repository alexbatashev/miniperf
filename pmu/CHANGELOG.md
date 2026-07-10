# Changelog

All notable changes to `miniperf-pmu` are recorded here. This project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): incompatible public
API changes require a major release after 1.0; before 1.0, incompatible changes
increment the minor version. Deprecations are preferred for at least one minor
release when practical.

## [Unreleased]

- Added AArch64 EventTimer userspace PMUv3 reads through Linux's
  `kernel.perf_user_access` mmap protocol, with grouped-read fallback.

## [0.1.0] - 2026-07-10

- Added actionable perf capability and error reporting.
- Added Tiger Lake event data and derived metric support.
- Added grouped `EventTimer` measurements with x86_64 `rdpmc` fast reads.
- Added bounded, in-memory closure sampling and call-stack capture modes.
- Added optional dynamic symbol summaries and Criterion measurements.
