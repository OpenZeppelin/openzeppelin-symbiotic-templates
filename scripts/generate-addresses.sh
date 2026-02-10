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

SOURCE_RPC_URL="${SOURCE_RPC_URL:-http://localhost:8545}"
DEST_RPC_URL="${DEST_RPC_URL:-http://localhost:8546}"

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
CCV_MODE=""

TEST_OAPP_SOURCE_ADDRESS=""
TEST_OAPP_DEST_ADDRESS=""
LZ_ENDPOINT_SOURCE_ADDRESS=""
LZ_ENDPOINT_DEST_ADDRESS=""
LZ_SOURCE_EID=""
LZ_DEST_EID=""

case "$ACTIVE_PROVIDER" in
    layerzero)
        req_file "$DEPLOY_DATA_DIR/source_contracts.json"
        req_file "$DEPLOY_DATA_DIR/dest_contracts.json"

        SOURCE_CHAIN_ID="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/source_contracts.json")"
        DEST_CHAIN_ID="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/dest_contracts.json")"
        DVN_SOURCE_ADDRESS="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/source_contracts.json")"
        DVN_DEST_ADDRESS="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/dest_contracts.json")"
        SEND_ULN_ADDRESS="$(jq -er '.sendUln' "$DEPLOY_DATA_DIR/source_contracts.json")"
        RECEIVE_ULN_ADDRESS="$(jq -er '.receiveUln' "$DEPLOY_DATA_DIR/dest_contracts.json")"
        SETTLEMENT_ADDRESS="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/dest_contracts.json")"

        if [[ -f "$DEPLOY_DATA_DIR/testoapp_source.json" ]]; then
            TEST_OAPP_SOURCE_ADDRESS="$(jq -er '.testOApp' "$DEPLOY_DATA_DIR/testoapp_source.json")"
        fi
        if [[ -f "$DEPLOY_DATA_DIR/testoapp_dest.json" ]]; then
            TEST_OAPP_DEST_ADDRESS="$(jq -er '.testOApp' "$DEPLOY_DATA_DIR/testoapp_dest.json")"
        fi
        if [[ -f "$DEPLOY_DATA_DIR/layerzero_source.json" ]]; then
            LZ_ENDPOINT_SOURCE_ADDRESS="$(jq -er '.endpoint' "$DEPLOY_DATA_DIR/layerzero_source.json")"
            LZ_SOURCE_EID="$(jq -er '.eid' "$DEPLOY_DATA_DIR/layerzero_source.json")"
        fi
        if [[ -f "$DEPLOY_DATA_DIR/layerzero_dest.json" ]]; then
            LZ_ENDPOINT_DEST_ADDRESS="$(jq -er '.endpoint' "$DEPLOY_DATA_DIR/layerzero_dest.json")"
            LZ_DEST_EID="$(jq -er '.eid' "$DEPLOY_DATA_DIR/layerzero_dest.json")"
        fi
        ;;
    chainlink_ccv)
        req_file "$DEPLOY_DATA_DIR/ccv_source_contracts.json"
        req_file "$DEPLOY_DATA_DIR/ccv_dest_contracts.json"

        SOURCE_CHAIN_ID="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
        DEST_CHAIN_ID="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
        CCV_SOURCE_ADDRESS="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
        CCV_DEST_ADDRESS="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
        CCV_SOURCE_SETTLEMENT_ADDRESS="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
        CCV_DEST_SETTLEMENT_ADDRESS="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"

        SETTLEMENT_ADDRESS="$CCV_DEST_SETTLEMENT_ADDRESS"
        CCV_MODE="$(get_ccv_mode)"
        if [[ "$CCV_MODE" != "symbiotic_mock" ]]; then
            echo "ERROR: unsupported providers.chainlink_ccv.mode '$CCV_MODE' (expected symbiotic_mock)" >&2
            exit 1
        fi
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
CCV_MODE=$CCV_MODE

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
