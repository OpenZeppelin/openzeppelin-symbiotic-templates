#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
COMPOSE_FILES="${COMPOSE_FILES:-}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi

# Load .env early so SOURCE_RPC_URL, DEST_RPC_URL, PRIVATE_KEY are available
# to common.sh and all downstream scripts.
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

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

run_startup_preflight() {
    echo "Running startup preflight checks..."
    ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE" ./scripts/preflight-start.sh
}

start_provider_services() {
    local active_provider="$1"
    local force_recreate_relayer="${2:-0}"
    if [[ "$force_recreate_relayer" == "1" ]]; then
        FORCE_RECREATE_RELAYER=1 COMPOSE_FILES="$COMPOSE_FILES" ./scripts/start-services.sh "$active_provider"
    else
        COMPOSE_FILES="$COMPOSE_FILES" ./scripts/start-services.sh "$active_provider"
    fi
}

# Cache relay infra addresses to config/networks/relay-infra.json for reuse across make clean
_cache_relay_infra() {
    local relay_cache="$PROJECT_ROOT/config/networks/relay-infra.json"
    local relay_data="$PROJECT_ROOT/contracts/deploy-data/relay_infra.json"

    [[ -f "$relay_data" ]] || return 0

    local chain_id
    chain_id="$(jq -r '.chainId' "$relay_data" 2>/dev/null)"
    [[ -n "$chain_id" && "$chain_id" != "null" ]] || return 0

    # Initialize cache file if missing
    if [[ ! -f "$relay_cache" ]]; then
        echo '{}' > "$relay_cache"
    fi

    # Merge new relay infra into cache keyed by chain ID
    local tmp
    tmp="$(mktemp)"
    jq --arg chain "$chain_id" --slurpfile infra "$relay_data" \
        '.[$chain] = $infra[0]' "$relay_cache" > "$tmp"
    mv "$tmp" "$relay_cache"

    echo "        ✓ Relay infra cached in config/networks/relay-infra.json (chain $chain_id)"
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

    # Extra forge flags for external networks
    local slow_flag=""
    if ! is_local; then
        slow_flag="--slow"
    fi

    (
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

    if is_local; then
        echo "      Phase 1: LayerZero mock deploy + Relay infra..."
        forge script script/DeployLayerZero.s.sol:DeployLayerZero \
            --sig "deploySource(uint32)" "$source_eid" \
            --rpc-url "$SOURCE_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
        echo "        ✓ LayerZero source"

        forge script script/DeployLayerZero.s.sol:DeployLayerZero \
            --sig "deployDest(uint32)" "$dest_eid" \
            --rpc-url "$DEST_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
        echo "        ✓ LayerZero dest"
    else
        echo "      Phase 1: Using pre-deployed LayerZero V2 endpoints..."
        local source_chain_id dest_chain_id
        source_chain_id="$(jq -er '.providers.layerzero.source_chain_id | numbers' "$ROOT_CONFIG_FILE")"
        dest_chain_id="$(jq -er '.providers.layerzero.destination_chain_id | numbers' "$ROOT_CONFIG_FILE")"
        local lz_endpoints="$PROJECT_ROOT/config/networks/layerzero-endpoints.json"

        # Fail fast if chain IDs are not in the endpoints reference
        jq -e --argjson chain "$source_chain_id" '.[$chain | tostring]' "$lz_endpoints" >/dev/null 2>&1 || {
            echo "ERROR: source chain ID $source_chain_id not found in $lz_endpoints" >&2
            echo "       Add the chain's LayerZero V2 addresses to that file and retry." >&2
            exit 1
        }
        jq -e --argjson chain "$dest_chain_id" '.[$chain | tostring]' "$lz_endpoints" >/dev/null 2>&1 || {
            echo "ERROR: destination chain ID $dest_chain_id not found in $lz_endpoints" >&2
            echo "       Add the chain's LayerZero V2 addresses to that file and retry." >&2
            exit 1
        }

        # Generate synthetic layerzero_source.json from pre-deployed addresses
        jq --argjson chain "$source_chain_id" --argjson eid "$source_eid" \
            '.[($chain | tostring)] | {
                chainId: $chain,
                eid: $eid,
                endpoint: .endpoint,
                sendUln: .sendUln302
            }' "$lz_endpoints" > deploy-data/layerzero_source.json
        echo "        ✓ LayerZero source endpoints (pre-deployed)"

        # Generate synthetic layerzero_dest.json from pre-deployed addresses
        jq --argjson chain "$dest_chain_id" --argjson eid "$dest_eid" \
            '.[($chain | tostring)] | {
                chainId: $chain,
                eid: $eid,
                endpoint: .endpoint,
                receiveUln: .receiveUln302
            }' "$lz_endpoints" > deploy-data/layerzero_dest.json
        echo "        ✓ LayerZero dest endpoints (pre-deployed)"
    fi

    # On external networks, point the script at pre-deployed Symbiotic Core addresses
    local core_config_env=""
    if ! is_local; then
        core_config_env="SYMBIOTIC_CORE_CONFIG=$PROJECT_ROOT/config/networks/symbiotic-core.json"
    fi

    # Check if relay infra can be reused from a previous deployment
    local relay_infra_reused=0
    if ! is_local && [[ "${FORCE_RELAY_DEPLOY:-0}" != "1" ]]; then
        local relay_cache="$PROJECT_ROOT/config/networks/relay-infra.json"
        # dest_chain_id already set above in the LZ endpoints block

        if [[ -f "$relay_cache" ]]; then
            local cached_settlement
            cached_settlement="$(jq -r --arg chain "$dest_chain_id" '.[$chain].settlement // empty' "$relay_cache" 2>/dev/null)"

            if [[ -n "$cached_settlement" && "$cached_settlement" != "null" ]]; then
                # Verify contract still exists on-chain
                local code
                code="$(cast code "$cached_settlement" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0x")"

                if [[ "$code" != "0x" && -n "$code" ]]; then
                    echo "        Reusing existing relay infra on chain $dest_chain_id (settlement: $cached_settlement)"
                    # Restore relay_infra.json from cache
                    jq --arg chain "$dest_chain_id" '.[$chain]' "$relay_cache" > deploy-data/relay_infra.json
                    relay_infra_reused=1
                else
                    echo "        Cached relay infra not found on-chain, deploying fresh..."
                fi
            fi
        fi
    fi

    if [[ "$relay_infra_reused" == "0" ]]; then
        env $core_config_env forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
            --rpc-url "$DEST_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --code-size-limit 50000 \
            --gas-estimate-multiplier 150 \
            --slow \
            --quiet
        echo "        ✓ Relay infra deployed"

        # On external networks, register operators separately
        if ! is_local; then
            echo "      Registering operators on external network..."

            local staking_token
            staking_token="$(jq -r '.stakingToken' deploy-data/relay_infra.json)"

            local base_key="${OPERATOR_BASE_KEY:-1000000000000000000}"

            # Phase 1: Fund operators with ETH + staking tokens (cast for reliability)
            for i in 0 1 2; do
                local op_addr
                op_addr=$(cast wallet address --private-key "$(printf "0x%064x" $((base_key + i)))")

                # Fund ETH
                cast send "$op_addr" --value 0.01ether \
                    --rpc-url "$DEST_RPC" --private-key "$PRIVATE_KEY" \
                    --confirmations 1 >/dev/null 2>&1
                # Transfer staking tokens
                cast send "$staking_token" "transfer(address,uint256)" "$op_addr" "100000000000000000000000" \
                    --rpc-url "$DEST_RPC" --private-key "$PRIVATE_KEY" \
                    --confirmations 1 >/dev/null 2>&1
                echo "        ✓ Operator $i funded ($op_addr)"
            done

            # Phase 2: Register each operator
            for i in 0 1 2; do
                env $core_config_env forge script script/RegisterOperators.s.sol:RegisterOperators \
                    --sig "registerOperator(uint256)" "$i" \
                    --rpc-url "$DEST_RPC" \
                    --broadcast \
                    --private-key "$PRIVATE_KEY" \
                    --slow \
                    --quiet
                echo "        ✓ Operator $i registered"
            done

            # Cache relay infra for future reuse
            _cache_relay_infra
        fi
    else
        echo "        ✓ Relay infra reused (skipped deploy + operator registration)"
    fi

    echo "      Phase 2: DVN (needs LZ + Settlement addresses)..."
    local send_uln receive_uln settlement_addr
    send_uln="$(jq -r '.sendUln' deploy-data/layerzero_source.json)"
    receive_uln="$(jq -r '.receiveUln' deploy-data/layerzero_dest.json)"
    settlement_addr="$(jq -r '.settlement' deploy-data/relay_infra.json)"

    forge script script/DeployDVN.s.sol:DeployDVN \
        --sig "deploySource(address,uint32)" "$send_uln" "$source_eid" \
        --rpc-url "$SOURCE_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ DVN source"

    forge script script/DeployDVN.s.sol:DeployDVN \
        --sig "deployDest(address,address,uint32)" "$receive_uln" "$settlement_addr" "$dest_eid" \
        --rpc-url "$DEST_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ DVN dest"

    echo "      Phase 3: Configure ULN with DVN..."
    local src_dvn dst_dvn
    src_dvn="$(jq -r '.dvn' deploy-data/source_contracts.json)"
    dst_dvn="$(jq -r '.dvn' deploy-data/dest_contracts.json)"

    if is_local; then
        # Local: set ULN defaults on mocks (applies to all OApps, no OApp address needed)
        forge script script/DeployLayerZero.s.sol:DeployLayerZero \
            --sig "configureSource(address,uint32)" "$src_dvn" "$dest_eid" \
            --rpc-url "$SOURCE_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
        echo "        ✓ Source ULN configured (mock)"

        forge script script/DeployLayerZero.s.sol:DeployLayerZero \
            --sig "configureDest(address,uint32)" "$dst_dvn" "$source_eid" \
            --rpc-url "$DEST_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
        echo "        ✓ Dest ULN configured (mock)"
    else
        # External: per-OApp config requires TestOApp address -- deferred to Phase 5
        echo "        (deferred to Phase 5 -- requires TestOApp addresses)"
    fi

    echo "      Phase 4: TestOApp..."
    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "deploySourceFromJson()" \
        --rpc-url "$SOURCE_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ TestOApp source"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "deployDestFromJson()" \
        --rpc-url "$DEST_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ TestOApp dest"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "configurePeersFromJson()" \
        --rpc-url "$SOURCE_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ Source peers configured"

    forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
        --sig "configurePeersFromJson()" \
        --rpc-url "$DEST_RPC" \
        --broadcast \
        --private-key "$PRIVATE_KEY" \
        $slow_flag \
        --quiet
    echo "        ✓ Dest peers configured"

    if ! is_local; then
        echo "      Phase 5: Configure OApp ULN on external endpoints..."
        # On real LZ V2, we configure per-OApp (can't set defaults on pre-deployed ULNs).
        # The deployer is already a delegate of the TestOApp (set in OApp constructor),
        # so the endpoint accepts these calls from the deployer on behalf of the OApp.
        local src_oapp dst_oapp
        src_oapp="$(jq -r '.testOApp' deploy-data/testoapp_source.json)"
        dst_oapp="$(jq -r '.testOApp' deploy-data/testoapp_dest.json)"

        forge script script/ConfigureExternalOApp.s.sol:ConfigureExternalOApp \
            --sig "configureSource(address,address,uint32)" "$src_oapp" "$src_dvn" "$dest_eid" \
            --rpc-url "$SOURCE_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            $slow_flag \
            --quiet
        echo "        ✓ Source OApp ULN configured (external)"

        forge script script/ConfigureExternalOApp.s.sol:ConfigureExternalOApp \
            --sig "configureDest(address,address,uint32)" "$dst_oapp" "$dst_dvn" "$source_eid" \
            --rpc-url "$DEST_RPC" \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            $slow_flag \
            --quiet
        echo "        ✓ Dest OApp ULN configured (external)"
    fi

    )

    cp contracts/deploy-data/source_contracts.json data/deploy-data/
    cp contracts/deploy-data/dest_contracts.json data/deploy-data/
    cp contracts/deploy-data/layerzero_source.json data/deploy-data/
    cp contracts/deploy-data/layerzero_dest.json data/deploy-data/
    cp contracts/deploy-data/testoapp_source.json data/deploy-data/
    cp contracts/deploy-data/testoapp_dest.json data/deploy-data/
    cp contracts/deploy-data/relay_infra.json data/deploy-data/
    ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE" DEPLOY_DATA_DIR="$PROJECT_ROOT/data/deploy-data" ./scripts/update-deploy-state.sh layerzero
    rm -f \
        data/deploy-data/source_contracts.json \
        data/deploy-data/dest_contracts.json \
        data/deploy-data/layerzero_source.json \
        data/deploy-data/layerzero_dest.json \
        data/deploy-data/testoapp_source.json \
        data/deploy-data/testoapp_dest.json \
        data/deploy-data/ccv_source_contracts.json \
        data/deploy-data/ccv_dest_contracts.json \
        data/deploy-data/relay_infra_source.json

    if is_local; then
        echo ""
        echo "      Mining blocks to finalize deposits..."
        cast rpc evm_mine --rpc-url "$SOURCE_RPC" >/dev/null 2>&1
        cast rpc evm_mine --rpc-url "$DEST_RPC" >/dev/null 2>&1
        echo "      ✓ Blocks mined"
    fi
}

resume_existing_deployment() {
    local active_provider="$1"

    echo "═══ Deploy artifacts already exist for ${active_provider}, regenerating configs... ═══"
    run_make configure ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE"

    if is_local; then
        echo "Refreshing settlement epoch for local devnet..."
        run_make refresh-epoch

        echo "Resetting runtime state for deterministic restart..."
        run_make reset-runtime
    fi

    run_startup_preflight

    echo "Starting services..."
    start_provider_services "$active_provider" 1

    echo "Reloading config-driven services (oz-monitor + operators)..."
    docker compose $COMPOSE_FILES --profile dev up -d --force-recreate oz-monitor operator-1 operator-2 operator-3 >/dev/null
    COMPOSE_FILES="$COMPOSE_FILES" ./scripts/start-services.sh "$active_provider" --wait-only >/dev/null
    echo "      ✓ Monitor/operators reloaded"

    maybe_configure_ccv_contracts "$active_provider"
}

first_run_deploy() {
    local active_provider="$1"

    echo "═══ First run for ${active_provider}: full deployment ═══"
    echo ""

    if is_local; then
        echo "[1/7] Building + starting chains (parallel)..."
        (cd contracts && forge build --quiet && echo "      ✓ Contracts compiled") &
        local build_pid=$!
        (docker compose $COMPOSE_FILES --profile dev build --quiet operator-1 >/dev/null 2>&1 && echo "      ✓ Operator image built") &
        local image_pid=$!
        (docker compose $COMPOSE_FILES --profile infra up -d --remove-orphans >/dev/null 2>&1 && echo "      ✓ Chains starting") &
        local chains_pid=$!
        wait_all_or_fail "$build_pid" "$image_pid" "$chains_pid"

        echo ""
        echo "[2/7] Waiting for chains..."
        wait_for_rpc "$SOURCE_RPC" "anvil" &
        local anvil_pid=$!
        wait_for_rpc "$DEST_RPC" "anvil-settlement" &
        local settlement_pid=$!
        wait_all_or_fail "$anvil_pid" "$settlement_pid"
    else
        echo "[1/7] Building contracts + operator image (parallel)..."
        (cd contracts && forge build --quiet && echo "      ✓ Contracts compiled") &
        local build_pid=$!
        (docker compose $COMPOSE_FILES --profile dev build --quiet operator-1 >/dev/null 2>&1 && echo "      ✓ Operator image built") &
        local image_pid=$!
        wait_all_or_fail "$build_pid" "$image_pid"

        echo ""
        echo "[2/7] Verifying external RPC connectivity..."
        wait_for_rpc "$SOURCE_RPC" "source chain" &
        local source_pid=$!
        wait_for_rpc "$DEST_RPC" "destination chain" &
        local dest_pid=$!
        wait_all_or_fail "$source_pid" "$dest_pid"
    fi

    echo ""
    echo "[3/7] Deploying contracts..."
    deploy_provider_contracts "$active_provider"

    echo ""
    echo "[4/7] Generating genesis valset..."
    ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE" ./scripts/generate-genesis.sh
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

    local active_provider
    [[ -f "$ROOT_CONFIG_FILE" ]] || {
        echo "ERROR: missing root config: $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    active_provider="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE" 2>/dev/null)" || {
        echo "ERROR: invalid root config: expected .active_provider in $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    if provider_has_deploy_state "$active_provider"; then
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
