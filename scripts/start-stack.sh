#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi

LZ_MARKER_FILE="$PROJECT_ROOT/data/deploy-data/relay-infra-complete.marker"
CCV_MARKER_FILE="$PROJECT_ROOT/data/deploy-data/ccv-complete.marker"

run_make() {
    (cd "$PROJECT_ROOT" && make "$@")
}

wait_all_or_fail() {
    local pids=("$@")
    local failed=0
    local pid

    for pid in "${pids[@]}"; do
        if ! wait "$pid"; then
            failed=1
        fi
    done

    return $failed
}

get_deploy_marker_file() {
    local active_provider="$1"
    case "$active_provider" in
        layerzero)
            echo "$LZ_MARKER_FILE"
            ;;
        chainlink_ccv)
            echo "$CCV_MARKER_FILE"
            ;;
        *)
            echo "ERROR: unsupported active_provider '$active_provider' in $ROOT_CONFIG_FILE" >&2
            exit 1
            ;;
    esac
}

run_startup_preflight() {
    echo "Running startup preflight checks..."
    ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE" ./scripts/preflight-start.sh
}

start_provider_services() {
    local active_provider="$1"
    local force_recreate_relayer="${2:-0}"
    if [[ "$force_recreate_relayer" == "1" ]]; then
        FORCE_RECREATE_RELAYER=1 ./scripts/start-services.sh "$active_provider"
    else
        ./scripts/start-services.sh "$active_provider"
    fi
}

maybe_configure_ccv_contracts() {
    local active_provider="$1"
    if [[ "$active_provider" == "chainlink_ccv" ]]; then
        echo "Applying SymbioticCCV remote-chain config..."
        run_make configure-ccv-contracts ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE"
    fi
}

wait_for_rpc() {
    local rpc_url="$1"
    local name="$2"
    local timeout=30
    local elapsed=0

    while ! cast client --rpc-url "$rpc_url" >/dev/null 2>&1; do
        sleep 1
        elapsed=$((elapsed + 1))
        if [[ $elapsed -ge $timeout ]]; then
            echo "      ERROR: Timeout waiting for ${name}" >&2
            return 1
        fi
    done

    echo "      ✓ ${name} ready"
}

deploy_provider_contracts() {
    local active_provider="$1"
    case "$active_provider" in
        layerzero)
            deploy_layerzero_contracts
            ;;
        chainlink_ccv)
            run_make deploy-ccv-contracts ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE"
            ;;
        *)
            echo "ERROR: unsupported active_provider '$active_provider' in $ROOT_CONFIG_FILE" >&2
            exit 1
            ;;
    esac
}

deploy_layerzero_contracts() {
    mkdir -p data/deploy-data contracts/deploy-data
    cd contracts

    local source_eid dest_eid
    source_eid="$(jq -er '.providers.layerzero.source_eid | numbers' "$ROOT_CONFIG_FILE" 2>/dev/null)" || {
        echo "ERROR: providers.layerzero.source_eid must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }
    dest_eid="$(jq -er '.providers.layerzero.destination_eid | numbers' "$ROOT_CONFIG_FILE" 2>/dev/null)" || {
        echo "ERROR: providers.layerzero.destination_eid must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    echo "      Phase 1: LayerZero + Relay infra..."
    forge script script/DeployLayerZero.s.sol:DeployLayerZero \
        --sig "deploySource(uint32)" "$source_eid" \
        --rpc-url http://localhost:8545 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ LayerZero source"

    forge script script/DeployLayerZero.s.sol:DeployLayerZero \
        --sig "deployDest(uint32)" "$dest_eid" \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ LayerZero dest"

    forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --code-size-limit 50000 \
        --gas-estimate-multiplier 150 \
        --slow \
        --quiet
    echo "        ✓ Relay infra (includes real Settlement)"

    echo "      Phase 2: DVN (needs LZ + Settlement addresses)..."
    local send_uln receive_uln settlement_addr
    send_uln="$(jq -r '.sendUln' deploy-data/layerzero_source.json)"
    receive_uln="$(jq -r '.receiveUln' deploy-data/layerzero_dest.json)"
    settlement_addr="$(jq -r '.settlement' deploy-data/relay_infra.json)"

    forge script script/DeployDVN.s.sol:DeployDVN \
        --sig "deploySource(address,uint32)" "$send_uln" "$source_eid" \
        --rpc-url http://localhost:8545 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ DVN source"

    forge script script/DeployDVN.s.sol:DeployDVN \
        --sig "deployDest(address,address,uint32)" "$receive_uln" "$settlement_addr" "$dest_eid" \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ DVN dest"

    echo "      Phase 3: Configure ULN with DVN..."
    local src_dvn dst_dvn
    src_dvn="$(jq -r '.dvn' deploy-data/source_contracts.json)"
    dst_dvn="$(jq -r '.dvn' deploy-data/dest_contracts.json)"

    forge script script/DeployLayerZero.s.sol:DeployLayerZero \
        --sig "configureSource(address,uint32)" "$src_dvn" "$dest_eid" \
        --rpc-url http://localhost:8545 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ Source ULN configured"

    forge script script/DeployLayerZero.s.sol:DeployLayerZero \
        --sig "configureDest(address,uint32)" "$dst_dvn" "$source_eid" \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ Dest ULN configured"

    echo "      Phase 4: TestOApp..."
    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "deploySourceFromJson()" \
        --rpc-url http://localhost:8545 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ TestOApp source"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "deployDestFromJson()" \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ TestOApp dest"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "configurePeersFromJson()" \
        --rpc-url http://localhost:8545 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ Source peers configured"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "configurePeersFromJson()" \
        --rpc-url http://localhost:8546 \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        --quiet
    echo "        ✓ Dest peers configured"

    cd "$PROJECT_ROOT"
    cp contracts/deploy-data/*.json data/deploy-data/
    date > data/deploy-data/deployment-complete.marker
    date > "$LZ_MARKER_FILE"

    echo ""
    echo "      Mining blocks to finalize deposits..."
    cast rpc evm_mine --rpc-url http://localhost:8545 >/dev/null 2>&1
    cast rpc evm_mine --rpc-url http://localhost:8546 >/dev/null 2>&1
    echo "      ✓ Blocks mined"
}

resume_existing_deployment() {
    local active_provider="$1"

    echo "═══ Deploy artifacts already exist for ${active_provider}, regenerating configs... ═══"
    run_make configure ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE"

    echo "Refreshing settlement epoch for local devnet..."
    run_make refresh-epoch

    echo "Resetting runtime state for deterministic restart..."
    run_make reset-runtime

    run_startup_preflight

    echo "Starting services..."
    start_provider_services "$active_provider" 1

    echo "Reloading config-driven services (oz-monitor + operators)..."
    docker compose --profile dev up -d --force-recreate oz-monitor operator-1 operator-2 operator-3 >/dev/null
    ./scripts/start-services.sh "$active_provider" --wait-only >/dev/null
    echo "      ✓ Monitor/operators reloaded"

    maybe_configure_ccv_contracts "$active_provider"
}

first_run_deploy() {
    local active_provider="$1"

    echo "═══ First run for ${active_provider}: full deployment ═══"
    echo ""
    echo "[1/7] Building + starting chains (parallel)..."
    (cd contracts && forge build --quiet && echo "      ✓ Contracts compiled") &
    local build_pid=$!
    (docker compose --profile dev build --quiet operator-1 >/dev/null 2>&1 && echo "      ✓ Operator image built") &
    local image_pid=$!
    (docker compose --profile infra up -d --remove-orphans >/dev/null 2>&1 && echo "      ✓ Chains starting") &
    local chains_pid=$!
    wait_all_or_fail "$build_pid" "$image_pid" "$chains_pid"

    echo ""
    echo "[2/7] Waiting for chains..."
    wait_for_rpc "http://localhost:8545" "anvil" &
    local anvil_pid=$!
    wait_for_rpc "http://localhost:8546" "anvil-settlement" &
    local settlement_pid=$!
    wait_all_or_fail "$anvil_pid" "$settlement_pid"

    echo ""
    echo "[3/7] Deploying contracts..."
    deploy_provider_contracts "$active_provider"

    echo ""
    echo "[4/7] Generating genesis valset..."
    ./scripts/generate-genesis.sh
    echo "      ✓ Genesis committed"

    echo ""
    echo "[5/7] Generating configs..."
    run_make configure ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE"

    echo ""
    echo "[6/7] Startup preflight checks..."
    run_startup_preflight

    echo ""
    echo "[7/7] Starting services..."
    start_provider_services "$active_provider"
    maybe_configure_ccv_contracts "$active_provider"
    echo "      ✓ All services started"
}

main() {
    cd "$PROJECT_ROOT"

    local active_provider deploy_marker
    [[ -f "$ROOT_CONFIG_FILE" ]] || {
        echo "ERROR: missing root config: $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    active_provider="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE" 2>/dev/null)" || {
        echo "ERROR: invalid root config: expected .active_provider in $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    deploy_marker="$(get_deploy_marker_file "$active_provider")"

    if [[ -f "$deploy_marker" ]]; then
        resume_existing_deployment "$active_provider"
    else
        first_run_deploy "$active_provider"
    fi

    echo ""
    echo "═══════════════════════════════════════════════════════════════════"
    echo "Stack started! Run 'make status' to check health."
    echo "═══════════════════════════════════════════════════════════════════"
}

main "$@"
