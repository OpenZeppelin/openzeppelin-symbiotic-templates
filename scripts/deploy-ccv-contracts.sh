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

        local ccv_source_selector ccv_dest_selector
        ccv_source_selector="$(jq -r '.providers.chainlink_ccv.source_chain_selector // 31337' "$ROOT_CONFIG_FILE_ABS")"
        ccv_dest_selector="$(jq -r '.providers.chainlink_ccv.destination_chain_selector // 31338' "$ROOT_CONFIG_FILE_ABS")"

        forge build --quiet

        local needs_relay_infra_source=1
        local needs_relay_infra_dest=1
        if [[ -f deploy-data/relay_infra_source.json ]]; then
            local existing_settlement_source existing_code_source
            existing_settlement_source="$(jq -r '.settlement // empty' deploy-data/relay_infra_source.json)"
            if [[ -n "$existing_settlement_source" && "$existing_settlement_source" != "null" ]]; then
                existing_code_source="$(cast code "$existing_settlement_source" --rpc-url http://localhost:8545 2>/dev/null || echo 0x)"
                if [[ "$existing_code_source" != "0x" ]]; then
                    needs_relay_infra_source=0
                fi
            fi
        fi

        if [[ -f deploy-data/relay_infra.json ]]; then
            local existing_settlement_dest existing_code_dest
            existing_settlement_dest="$(jq -r '.settlement // empty' deploy-data/relay_infra.json)"
            if [[ -n "$existing_settlement_dest" && "$existing_settlement_dest" != "null" ]]; then
                existing_code_dest="$(cast code "$existing_settlement_dest" --rpc-url http://localhost:8546 2>/dev/null || echo 0x)"
                if [[ "$existing_code_dest" != "0x" ]]; then
                    needs_relay_infra_dest=0
                fi
            fi
        fi

        local dest_backup_file=""
        if [[ $needs_relay_infra_source -eq 1 && $needs_relay_infra_dest -eq 0 && -f deploy-data/relay_infra.json ]]; then
            dest_backup_file="deploy-data/relay_infra_dest_backup.json"
            cp deploy-data/relay_infra.json "$dest_backup_file"
        fi

        if [[ $needs_relay_infra_source -eq 1 ]]; then
            echo "Deploying relay infrastructure on source chain..."
            forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
                --rpc-url http://localhost:8545 \
                --broadcast \
                --private-key "$PRIVATE_KEY" \
                --code-size-limit 50000 \
                --gas-estimate-multiplier 150 \
                --slow \
                --quiet
            cp deploy-data/relay_infra.json deploy-data/relay_infra_source.json

            if [[ -n "$dest_backup_file" && -f "$dest_backup_file" ]]; then
                cp "$dest_backup_file" deploy-data/relay_infra.json
                rm -f "$dest_backup_file"
            fi
        fi

        if [[ $needs_relay_infra_dest -eq 1 ]]; then
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

        # Keep source relay infra in a dedicated file if destination deployment overwrote relay_infra.json.
        if [[ ! -f deploy-data/relay_infra_source.json && -f deploy-data/relay_infra.json ]]; then
            local existing_settlement
            existing_settlement="$(jq -r '.settlement // empty' deploy-data/relay_infra.json)"
            if [[ -n "$existing_settlement" && "$existing_settlement" != "null" ]]; then
                local existing_code
                existing_code="$(cast code "$existing_settlement" --rpc-url http://localhost:8545 2>/dev/null || echo 0x)"
                if [[ "$existing_code" != "0x" ]]; then
                    cp deploy-data/relay_infra.json deploy-data/relay_infra_source.json
                fi
            fi
        fi

        local source_settlement_addr settlement_addr
        source_settlement_addr="$(jq -r '.settlement' deploy-data/relay_infra_source.json)"
        settlement_addr="$(jq -r '.settlement' deploy-data/relay_infra.json)"
        [[ -n "$source_settlement_addr" && "$source_settlement_addr" != "null" ]] || die "missing source settlement in deploy-data/relay_infra_source.json"
        [[ -n "$settlement_addr" && "$settlement_addr" != "null" ]] || die "missing destination settlement in deploy-data/relay_infra.json"

        forge script script/DeployCCV.s.sol:DeployCCV \
            --sig "deploySource(address,uint64)" "$source_settlement_addr" "$ccv_dest_selector" \
            --rpc-url http://localhost:8545 \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet

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
    if [[ -f "$PROJECT_ROOT/contracts/deploy-data/relay_infra_source.json" ]]; then
        cp "$PROJECT_ROOT/contracts/deploy-data/relay_infra_source.json" "$PROJECT_ROOT/data/deploy-data/"
    fi
    if [[ -f "$PROJECT_ROOT/contracts/deploy-data/relay-infra-complete.marker" ]]; then
        cp "$PROJECT_ROOT/contracts/deploy-data/relay-infra-complete.marker" "$PROJECT_ROOT/data/deploy-data/"
    fi
    date > "$PROJECT_ROOT/data/deploy-data/ccv-complete.marker"
    echo "✓ SymbioticCCV deploy artifacts written to data/deploy-data/"
}

main "$@"
