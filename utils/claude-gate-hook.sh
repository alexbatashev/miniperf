#!/bin/sh
# Claude Code PreToolUse hook: run the profiler gate before any git commit/push.
cmd=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("tool_input",{}).get("command",""))')
case "$cmd" in
  *"git commit"*|*"git push"*) ;;
  *) exit 0 ;;
esac
if ! "$(dirname "$0")/gate.sh" >/tmp/mperf-gate.log 2>&1; then
  echo "profiler gate failed; commit refused. See /tmp/mperf-gate.log:" >&2
  tail -40 /tmp/mperf-gate.log >&2
  exit 2
fi
