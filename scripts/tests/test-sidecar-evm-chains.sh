#!/usr/bin/env bash
# Regression test for the symbiotic relay sidecar entrypoint: `--evm.chains`
# must always include BOTH source and dest RPC URLs, comma-separated, on every
# chain (not just local). Previously a non-local branch passed only DEST_RPC,
# which silently broke proof aggregation against mainnet.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SIDECAR="${REPO_ROOT}/scripts/symbiotic-relay/start-sidecar.sh"

if [ ! -f "$SIDECAR" ]; then
    echo "start-sidecar.sh not found at $SIDECAR" >&2
    exit 1
fi

# Replace the final exec with a print so we can capture the would-be args.
patched="$(mktemp)"
trap 'rm -f "$patched"' EXIT
sed 's|^exec /app/relay_sidecar|echo ARGS:|' "$SIDECAR" > "$patched"
chmod +x "$patched"

run_case() {
    local label="$1" source_rpc="$2" dest_rpc="$3"
    local output
    output=$(
        DRIVER_ADDRESS=0x0000000000000000000000000000000000000001 \
        DRIVER_CHAIN_ID=11155111 \
        SIDECAR_SECRET_KEYS=stub \
        SOURCE_CHAIN_ID=84532 \
        EVM_SOURCE_RPC="$source_rpc" \
        EVM_DEST_RPC="$dest_rpc" \
        "$patched" 1
    )
    local expected="--evm.chains ${source_rpc},${dest_rpc}"
    if ! grep -F -- "$expected" <<<"$output" >/dev/null; then
        echo "FAIL ($label): expected '$expected' in output:" >&2
        echo "$output" >&2
        exit 1
    fi
    echo "ok: $label"
}

run_case "non-local mainnet" "https://base.example/rpc" "https://eth.example/rpc"
run_case "local anvil"       "http://anvil:8545"        "http://anvil-settlement:8546"

echo "all start-sidecar.sh --evm.chains tests passed"
