#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_DATA_DIR="$PROJECT_ROOT/data/deploy-data"
RELAY_INFRA_FILE="$DEPLOY_DATA_DIR/relay_infra.json"
CHAIN_WAIT_TIMEOUT_SECONDS="${CHAIN_WAIT_TIMEOUT_SECONDS:-60}"
MAX_EPOCH_VALIDITY_SECONDS="${MAX_EPOCH_VALIDITY_SECONDS:-7200}"
FRESHNESS_BUFFER_SECONDS="${FRESHNESS_BUFFER_SECONDS:-300}"
LOOKBACK_EPOCHS="${LOOKBACK_EPOCHS:-240}"

die() {
    echo "ERROR: $*" >&2
    exit 1
}

parse_uint() {
    local raw="${1:-0}"
    raw="${raw%% *}"
    raw="${raw//[$'\r\n\t']}"
    if [[ "$raw" =~ ^[0-9]+$ ]]; then
        echo "$raw"
    else
        echo "0"
    fi
}

wait_for_chain() {
    local rpc_url="$1"
    local name="$2"
    local elapsed=0

    until cast client --rpc-url "$rpc_url" >/dev/null 2>&1; do
        sleep 1
        elapsed=$((elapsed + 1))
        if [[ $elapsed -ge $CHAIN_WAIT_TIMEOUT_SECONDS ]]; then
            die "timeout waiting for ${name} at ${rpc_url}"
        fi
    done
}

command -v docker >/dev/null 2>&1 || die "docker is required"
command -v cast >/dev/null 2>&1 || die "cast is required"
command -v jq >/dev/null 2>&1 || die "jq is required"

[[ -f "$RELAY_INFRA_FILE" ]] || die "missing $RELAY_INFRA_FILE (deploy relay infrastructure first)"

DRIVER_ADDRESS="$(jq -r '.driver // empty' "$RELAY_INFRA_FILE")"
SETTLEMENT_ADDRESS="$(jq -r '.settlement // empty' "$RELAY_INFRA_FILE")"
[[ -n "$DRIVER_ADDRESS" ]] || die "missing driver in $RELAY_INFRA_FILE"
[[ -n "$SETTLEMENT_ADDRESS" ]] || die "missing settlement in $RELAY_INFRA_FILE"

echo "Ensuring infra chains are running..."
docker compose --profile infra up -d --remove-orphans >/dev/null

echo "Waiting for source + settlement chains..."
wait_for_chain "http://localhost:8545" "anvil"
wait_for_chain "http://localhost:8546" "anvil-settlement"

current_epoch_raw="$(cast call "$DRIVER_ADDRESS" "getCurrentEpoch()(uint48)" --rpc-url http://localhost:8546 2>/dev/null || echo "0")"
current_epoch="$(parse_uint "$current_epoch_raw")"
[[ "$current_epoch" -gt 0 ]] || die "failed to read current epoch from driver $DRIVER_ADDRESS"

start_epoch=$((current_epoch - 1))
if [[ $start_epoch -lt 1 ]]; then
    start_epoch=1
fi

min_epoch=$((start_epoch - LOOKBACK_EPOCHS))
if [[ $min_epoch -lt 1 ]]; then
    min_epoch=1
fi

latest_committed_epoch=0
latest_capture_ts=0

for ((epoch=start_epoch; epoch>=min_epoch; epoch--)); do
    capture_raw="$(cast call "$SETTLEMENT_ADDRESS" "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)" "$epoch" --rpc-url http://localhost:8546 2>/dev/null || echo "0")"
    capture_ts="$(parse_uint "$capture_raw")"
    if [[ "$capture_ts" -gt 0 ]]; then
        latest_committed_epoch="$epoch"
        latest_capture_ts="$capture_ts"
        break
    fi
done

if [[ "$latest_capture_ts" -eq 0 ]]; then
    epoch_one_raw="$(cast call "$SETTLEMENT_ADDRESS" "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)" 1 --rpc-url http://localhost:8546 2>/dev/null || echo "0")"
    epoch_one_ts="$(parse_uint "$epoch_one_raw")"
    if [[ "$epoch_one_ts" -gt 0 ]]; then
        latest_committed_epoch=1
        latest_capture_ts="$epoch_one_ts"
    fi
fi

now_ts="$(date +%s)"
refresh_required=false
refresh_reason=""

if [[ "$latest_capture_ts" -eq 0 ]]; then
    refresh_required=true
    refresh_reason="no committed epoch found in settlement lookback window"
else
    age_seconds=$((now_ts - latest_capture_ts))
    freshness_threshold=$((MAX_EPOCH_VALIDITY_SECONDS - FRESHNESS_BUFFER_SECONDS))
    if [[ "$age_seconds" -ge "$freshness_threshold" ]]; then
        refresh_required=true
        refresh_reason="latest committed epoch $latest_committed_epoch is too old (age=${age_seconds}s, threshold=${freshness_threshold}s)"
    fi
fi

if [[ "$refresh_required" == "false" ]]; then
    echo "Settlement epoch is fresh (epoch=$latest_committed_epoch capture=$latest_capture_ts age=$((now_ts - latest_capture_ts))s)."
    exit 0
fi

echo "Refreshing committed settlement epoch: $refresh_reason"
FORCE_GENESIS=1 "$PROJECT_ROOT/scripts/generate-genesis.sh"
echo "Settlement epoch refresh complete."
