#!/usr/bin/env bash
set -euo pipefail
PID="$1"
if [[ -n "$PID" ]]; then
  kill "$PID" 2>/dev/null || true
fi
