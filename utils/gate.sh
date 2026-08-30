#!/bin/sh
# Runs the profiler against real hardware. Blocks commits when it fails.
set -e
cd "$(dirname "$0")/.."
exec cargo test -q -p mperf -p truth --test smoke --test profile
