#!/usr/bin/env bash
# Generate addresses.env from deployment data
#
# This script creates a shell-sourceable file with all deployed contract addresses.
# Run after deployment completes.
#
# Usage: ./scripts/generate-addresses.sh
#        make addresses

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DEPLOY_DATA_DIR="${DEPLOY_DATA_DIR:-$PROJECT_ROOT/data/deploy-data}"
OUT_ENV="${OUT_ENV:-$DEPLOY_DATA_DIR/addresses.env}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
DEPLOY_DATA="$DEPLOY_DATA_DIR"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

DEPLOY_STATE_FILE="$DEPLOY_DATA_DIR/deploy-state.json"

if is_local; then
    SOURCE_RPC_URL="${SOURCE_RPC_URL:-http://localhost:8545}"
    DEST_RPC_URL="${DEST_RPC_URL:-http://localhost:8546}"
else
    SOURCE_RPC_URL="${SOURCE_RPC_URL:-}"
    DEST_RPC_URL="${DEST_RPC_URL:-}"
fi

# Check dependencies
require() { command -v "$1" >/dev/null 2>&1 || { echo "ERROR: missing dependency: $1" >&2; exit 1; }; }
require jq

req_file() { [[ -f "$1" ]] || { echo "ERROR: missing file: $1" >&2; exit 1; }; }

if [[ ! -f "$ROOT_CONFIG_FILE" ]]; then
    echo "ERROR: missing root config: $ROOT_CONFIG_FILE" >&2
    exit 1
fi

ACTIVE_PROVIDER="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE")"

# Shared defaults
DVN_SOURCE_ADDRESS=""
DVN_DEST_ADDRESS=""
SEND_ULN_ADDRESS=""
RECEIVE_ULN_ADDRESS=""
SETTLEMENT_ADDRESS=""

CCV_SOURCE_ADDRESS=""
CCV_DEST_ADDRESS=""
CCV_SOURCE_SETTLEMENT_ADDRESS=""
CCV_DEST_SETTLEMENT_ADDRESS=""
CCV_SOURCE_ONRAMP_ADDRESS=""
CCV_SOURCE_OFFRAMP_ADDRESS=""
CCV_DEST_ONRAMP_ADDRESS=""
CCV_DEST_OFFRAMP_ADDRESS=""
CCV_SOURCE_CHAIN_SELECTOR=""
CCV_DEST_CHAIN_SELECTOR=""

TEST_OAPP_SOURCE_ADDRESS=""
TEST_OAPP_DEST_ADDRESS=""
LZ_ENDPOINT_SOURCE_ADDRESS=""
LZ_ENDPOINT_DEST_ADDRESS=""
LZ_SOURCE_EID=""
LZ_DEST_EID=""

case "$ACTIVE_PROVIDER" in
    layerzero)
        req_file "$DEPLOY_STATE_FILE"
        provider_has_deploy_state "layerzero" || {
            echo "ERROR: layerzero deploy state is incomplete: $DEPLOY_STATE_FILE" >&2
            exit 1
        }

        SOURCE_CHAIN_ID="$(jq -er '.providers.layerzero.source_chain_id | numbers' "$DEPLOY_STATE_FILE")"
        DEST_CHAIN_ID="$(jq -er '.providers.layerzero.destination_chain_id | numbers' "$DEPLOY_STATE_FILE")"
        DVN_SOURCE_ADDRESS="$(jq -er '.providers.layerzero.source.dvn' "$DEPLOY_STATE_FILE")"
        DVN_DEST_ADDRESS="$(jq -er '.providers.layerzero.destination.dvn' "$DEPLOY_STATE_FILE")"
        SEND_ULN_ADDRESS="$(jq -er '.providers.layerzero.source.send_uln' "$DEPLOY_STATE_FILE")"
        RECEIVE_ULN_ADDRESS="$(jq -er '.providers.layerzero.destination.receive_uln' "$DEPLOY_STATE_FILE")"
        SETTLEMENT_ADDRESS="$(jq -er '.providers.layerzero.destination.settlement' "$DEPLOY_STATE_FILE")"
        TEST_OAPP_SOURCE_ADDRESS="$(jq -er '.providers.layerzero.source.test_oapp' "$DEPLOY_STATE_FILE")"
        TEST_OAPP_DEST_ADDRESS="$(jq -er '.providers.layerzero.destination.test_oapp' "$DEPLOY_STATE_FILE")"
        LZ_ENDPOINT_SOURCE_ADDRESS="$(jq -er '.providers.layerzero.source.endpoint' "$DEPLOY_STATE_FILE")"
        LZ_ENDPOINT_DEST_ADDRESS="$(jq -er '.providers.layerzero.destination.endpoint' "$DEPLOY_STATE_FILE")"
        LZ_SOURCE_EID="$(jq -er '.providers.layerzero.source_eid | numbers' "$DEPLOY_STATE_FILE")"
        LZ_DEST_EID="$(jq -er '.providers.layerzero.destination_eid | numbers' "$DEPLOY_STATE_FILE")"
        ;;
    chainlink_ccv)
        req_file "$DEPLOY_STATE_FILE"
        provider_has_deploy_state "chainlink_ccv" || {
            echo "ERROR: chainlink_ccv deploy state is incomplete: $DEPLOY_STATE_FILE" >&2
            exit 1
        }

        SOURCE_CHAIN_ID="$(jq -er '.providers.chainlink_ccv.source_chain_id | numbers' "$DEPLOY_STATE_FILE")"
        DEST_CHAIN_ID="$(jq -er '.providers.chainlink_ccv.destination_chain_id | numbers' "$DEPLOY_STATE_FILE")"
        CCV_SOURCE_ADDRESS="$(jq -er '.providers.chainlink_ccv.source.ccv' "$DEPLOY_STATE_FILE")"
        CCV_DEST_ADDRESS="$(jq -er '.providers.chainlink_ccv.destination.ccv' "$DEPLOY_STATE_FILE")"
        CCV_SOURCE_SETTLEMENT_ADDRESS="$(jq -er '.providers.chainlink_ccv.source.settlement' "$DEPLOY_STATE_FILE")"
        CCV_DEST_SETTLEMENT_ADDRESS="$(jq -er '.providers.chainlink_ccv.destination.settlement' "$DEPLOY_STATE_FILE")"

        SETTLEMENT_ADDRESS="$CCV_DEST_SETTLEMENT_ADDRESS"
        CCV_SOURCE_CHAIN_SELECTOR="$(get_ccv_source_chain_selector)"
        CCV_DEST_CHAIN_SELECTOR="$(get_ccv_dest_chain_selector)"
        CCV_SOURCE_ONRAMP_ADDRESS="$(get_ccv_source_onramp_address 2>/dev/null || true)"
        CCV_SOURCE_OFFRAMP_ADDRESS="$(get_ccv_source_offramp_address 2>/dev/null || true)"
        CCV_DEST_ONRAMP_ADDRESS="$(get_ccv_dest_onramp_address 2>/dev/null || true)"
        CCV_DEST_OFFRAMP_ADDRESS="$(get_ccv_dest_offramp_address 2>/dev/null || true)"
        ;;
    *)
        echo "ERROR: unsupported active_provider '$ACTIVE_PROVIDER' in $ROOT_CONFIG_FILE" >&2
        exit 1
        ;;
esac

mkdir -p "$DEPLOY_DATA_DIR"

cat > "$OUT_ENV" <<EOF
# Generated by scripts/generate-addresses.sh
# $(date -u +"%Y-%m-%dT%H:%M:%SZ")
#
# Source this file for manual testing:
#   source data/deploy-data/addresses.env

# RPCs
SOURCE_RPC_URL=$SOURCE_RPC_URL
DEST_RPC_URL=$DEST_RPC_URL

# Active provider
ACTIVE_PROVIDER=$ACTIVE_PROVIDER

# Chains
SOURCE_CHAIN_ID=$SOURCE_CHAIN_ID
DEST_CHAIN_ID=$DEST_CHAIN_ID

# DVN
DVN_SOURCE_ADDRESS=$DVN_SOURCE_ADDRESS
DVN_DEST_ADDRESS=$DVN_DEST_ADDRESS

# ULN
SEND_ULN_ADDRESS=$SEND_ULN_ADDRESS
RECEIVE_ULN_ADDRESS=$RECEIVE_ULN_ADDRESS

# Settlement
SETTLEMENT_ADDRESS=$SETTLEMENT_ADDRESS

# Chainlink CCV
CCV_SOURCE_ADDRESS=$CCV_SOURCE_ADDRESS
CCV_DEST_ADDRESS=$CCV_DEST_ADDRESS
CCV_SOURCE_SETTLEMENT_ADDRESS=$CCV_SOURCE_SETTLEMENT_ADDRESS
CCV_DEST_SETTLEMENT_ADDRESS=$CCV_DEST_SETTLEMENT_ADDRESS
CCV_SOURCE_CHAIN_SELECTOR=$CCV_SOURCE_CHAIN_SELECTOR
CCV_DEST_CHAIN_SELECTOR=$CCV_DEST_CHAIN_SELECTOR
CCV_SOURCE_ONRAMP_ADDRESS=$CCV_SOURCE_ONRAMP_ADDRESS
CCV_SOURCE_OFFRAMP_ADDRESS=$CCV_SOURCE_OFFRAMP_ADDRESS
CCV_DEST_ONRAMP_ADDRESS=$CCV_DEST_ONRAMP_ADDRESS
CCV_DEST_OFFRAMP_ADDRESS=$CCV_DEST_OFFRAMP_ADDRESS

# TestOApp (for manual testing)
TEST_OAPP_SOURCE_ADDRESS=$TEST_OAPP_SOURCE_ADDRESS
TEST_OAPP_DEST_ADDRESS=$TEST_OAPP_DEST_ADDRESS

# LayerZero Endpoints
LZ_ENDPOINT_SOURCE_ADDRESS=$LZ_ENDPOINT_SOURCE_ADDRESS
LZ_ENDPOINT_DEST_ADDRESS=$LZ_ENDPOINT_DEST_ADDRESS
LZ_SOURCE_EID=$LZ_SOURCE_EID
LZ_DEST_EID=$LZ_DEST_EID
EOF

echo "Wrote $OUT_ENV"
