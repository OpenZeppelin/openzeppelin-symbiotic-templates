#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESET_SCRIPT="$REPO_ROOT/scripts/reset-runtime-state.sh"

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/data/sidecar-1" "$TMP_ROOT/data/sidecar-2" "$TMP_ROOT/data/sidecar-3" "$TMP_ROOT/data/oz-monitor"
echo "stale" > "$TMP_ROOT/data/sidecar-1/stale-proof.json"
echo "stale" > "$TMP_ROOT/data/sidecar-2/stale-proof.json"
echo "stale" > "$TMP_ROOT/data/sidecar-3/stale-proof.json"
echo "12345" > "$TMP_ROOT/data/oz-monitor/local_anvil_last_block.txt"

PROJECT_ROOT="$TMP_ROOT" SKIP_DOCKER_RESET=1 "$RESET_SCRIPT" >/dev/null

for dir in "$TMP_ROOT/data/sidecar-1" "$TMP_ROOT/data/sidecar-2" "$TMP_ROOT/data/sidecar-3"; do
    if [[ -n "$(find "$dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
        echo "expected sidecar runtime dir to be empty: $dir" >&2
        exit 1
    fi
done

if [[ -f "$TMP_ROOT/data/oz-monitor/local_anvil_last_block.txt" ]]; then
    echo "expected monitor cursor to be removed" >&2
    exit 1
fi

echo "reset-runtime test passed"
