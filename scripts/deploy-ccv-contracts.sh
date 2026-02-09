#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi
ROOT_CONFIG_FILE_ABS="$ROOT_CONFIG_FILE"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

die() {
    echo "ERROR: $*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"
}

require_file() {
    [[ -f "$1" ]] || die "missing file: $1"
}

main() {
    require_cmd jq
    require_cmd cast
    require_cmd forge

    if [[ ! -f "$PROJECT_ROOT/.env" ]]; then
        die ".env not found. Run 'make setup' first."
    fi
    require_file "$ROOT_CONFIG_FILE"

    if [[ "$(jq -r '.providers.chainlink_ccv.deployment.source_use_mock_settlement // true' "$ROOT_CONFIG_FILE")" != "true" ]] && \
       [[ -z "$(jq -r '.providers.chainlink_ccv.deployment.source_settlement_address // empty' "$ROOT_CONFIG_FILE")" ]]; then
        die "providers.chainlink_ccv.deployment.source_settlement_address is required when source_use_mock_settlement=false"
    fi

    local ccv_mode
    ccv_mode="$(get_ccv_mode)"
    if [[ "$ccv_mode" != "symbiotic_mock" ]]; then
        die "unsupported providers.chainlink_ccv.mode '$ccv_mode' (expected symbiotic_mock)"
    fi

    echo "Deploying SymbioticCCV contracts..."
    mkdir -p "$PROJECT_ROOT/data/deploy-data" "$PROJECT_ROOT/contracts/deploy-data"

    if ! cast client --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        echo "ERROR: source chain is not reachable at http://localhost:8545" >&2
        echo "Start infrastructure first (e.g. docker compose --profile infra up -d)" >&2
        exit 1
    fi
    if ! cast client --rpc-url http://localhost:8546 >/dev/null 2>&1; then
        echo "ERROR: destination chain is not reachable at http://localhost:8546" >&2
        echo "Start infrastructure first (e.g. docker compose --profile infra up -d)" >&2
        exit 1
    fi

    (
        cd "$PROJECT_ROOT/contracts"

        local ccv_source_use_mock ccv_source_settlement_address ccv_source_storage_location ccv_source_selector ccv_dest_storage_location
        ccv_source_use_mock="$(jq -r '.providers.chainlink_ccv.deployment.source_use_mock_settlement // true' "$ROOT_CONFIG_FILE_ABS")"
        ccv_source_settlement_address="$(jq -r '.providers.chainlink_ccv.deployment.source_settlement_address // empty' "$ROOT_CONFIG_FILE_ABS")"
        ccv_source_storage_location="$(jq -r '.providers.chainlink_ccv.deployment.source_storage_location // "mock://symbiotic-ccv/source"' "$ROOT_CONFIG_FILE_ABS")"
        ccv_source_selector="$(jq -r '.providers.chainlink_ccv.source_chain_selector // 31337' "$ROOT_CONFIG_FILE_ABS")"
        ccv_dest_storage_location="$(jq -r '.providers.chainlink_ccv.deployment.destination_storage_location // "mock://symbiotic-ccv/destination"' "$ROOT_CONFIG_FILE_ABS")"

        forge build --quiet

        local needs_relay_infra=1
        if [[ -f deploy-data/relay_infra.json ]]; then
            local existing_settlement existing_code
            existing_settlement="$(jq -r '.settlement // empty' deploy-data/relay_infra.json)"
            if [[ -n "$existing_settlement" && "$existing_settlement" != "null" ]]; then
                existing_code="$(cast code "$existing_settlement" --rpc-url http://localhost:8546 2>/dev/null || echo 0x)"
                if [[ "$existing_code" != "0x" ]]; then
                    needs_relay_infra=0
                fi
            fi
        fi

        if [[ $needs_relay_infra -eq 1 ]]; then
            echo "Deploying relay infrastructure on destination chain..."
            forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
                --rpc-url http://localhost:8546 \
                --broadcast \
                --private-key "$PRIVATE_KEY" \
                --code-size-limit 50000 \
                --gas-estimate-multiplier 150 \
                --slow \
                --quiet
        fi

        local settlement_addr
        settlement_addr="$(jq -r '.settlement' deploy-data/relay_infra.json)"

        CCV_SOURCE_USE_MOCK_SETTLEMENT="$ccv_source_use_mock" \
        CCV_SOURCE_SETTLEMENT_ADDRESS="$ccv_source_settlement_address" \
        CCV_SOURCE_STORAGE_LOCATION="$ccv_source_storage_location" \
        forge script script/DeployCCV.s.sol:DeployCCV \
            --sig "deploySource()" \
            --rpc-url http://localhost:8545 \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet

        CCV_DEST_STORAGE_LOCATION="$ccv_dest_storage_location" \
        forge script script/DeployCCV.s.sol:DeployCCV \
            --sig "deployDest(address,uint64)" "$settlement_addr" "$ccv_source_selector" \
            --rpc-url http://localhost:8546 \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
    )

    cp "$PROJECT_ROOT/contracts/deploy-data/ccv_source_contracts.json" "$PROJECT_ROOT/data/deploy-data/"
    cp "$PROJECT_ROOT/contracts/deploy-data/ccv_dest_contracts.json" "$PROJECT_ROOT/data/deploy-data/"
    if [[ -f "$PROJECT_ROOT/contracts/deploy-data/relay_infra.json" ]]; then
        cp "$PROJECT_ROOT/contracts/deploy-data/relay_infra.json" "$PROJECT_ROOT/data/deploy-data/"
    fi
    if [[ -f "$PROJECT_ROOT/contracts/deploy-data/relay-infra-complete.marker" ]]; then
        cp "$PROJECT_ROOT/contracts/deploy-data/relay-infra-complete.marker" "$PROJECT_ROOT/data/deploy-data/"
    fi
    date > "$PROJECT_ROOT/data/deploy-data/ccv-complete.marker"
    echo "✓ SymbioticCCV deploy artifacts written to data/deploy-data/"
}

main "$@"
