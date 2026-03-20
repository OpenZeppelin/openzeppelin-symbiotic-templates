#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILES="${COMPOSE_FILES:-}"
STACK_MODE="${STACK_MODE:-full}"

# Load .env early so SOURCE_RPC_URL, DEST_RPC_URL, PRIVATE_KEY are available
# to common.sh and all downstream scripts.
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Resolve environment config. ENV is also exported for docker-compose.yml
# variable substitution (operator volume mounts use ${ENV:-local}).
export ENV="${ENV:-local}"
export ENV_CONFIG="${ENV_CONFIG:-$PROJECT_ROOT/config/environments/${ENV}.json}"
if [[ "$ENV_CONFIG" != /* ]]; then
    ENV_CONFIG="$PROJECT_ROOT/$ENV_CONFIG"
    export ENV_CONFIG
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
    "$PROJECT_ROOT/scripts/preflight-start.sh"
}

run_runtime_validation() {
    local validate_managed=0

    if is_local || [[ "$STACK_MODE" == "services_only" ]]; then
        validate_managed="${VALIDATE_MANAGED_OPERATORS:-1}"
    fi

    VALIDATE_MANAGED_OPERATORS="$validate_managed" \
        "$PROJECT_ROOT/scripts/validate-env.sh"
}

genesis_refresh_needed() {
    local settlement epoch captured_ts now age

    settlement="$(env_deployment destination relayInfra.settlement 2>/dev/null || true)"
    if [[ -z "$settlement" || "$settlement" == "null" ]]; then
        return 0
    fi

    epoch="$(cast call "$settlement" "getLastCommittedHeaderEpoch()(uint48)" --rpc-url "$DEST_RPC" 2>/dev/null || true)"
    epoch="$(printf '%s' "$epoch" | tr -d '[:space:]')"
    if [[ -z "$epoch" || "$epoch" == "0" ]]; then
        return 0
    fi

    captured_ts="$(cast call "$settlement" "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)" "$epoch" --rpc-url "$DEST_RPC" 2>/dev/null || true)"
    captured_ts="$(printf '%s' "$captured_ts" | tr -d '[:space:]')"
    if [[ -z "$captured_ts" || ! "$captured_ts" =~ ^[0-9]+$ || "$captured_ts" == "0" ]]; then
        return 0
    fi

    now="$(date +%s)"
    age=$((now - captured_ts))
    (( age >= ${MAX_EPOCH_VALIDITY_SECONDS:-7200} ))
}

maybe_refresh_genesis() {
    is_local && return 0
    [[ "$STACK_MODE" == "services_only" ]] && return 0

    if genesis_refresh_needed; then
        echo "Refreshing settlement genesis before validation..."
        FORCE_GENESIS=1 "$PROJECT_ROOT/scripts/generate-genesis.sh"
    fi
}

ensure_external_relayer_keystores_match_operator_keys() {
    is_local && return 0

    local passphrase="${KEYSTORE_PASSPHRASE:-}"
    if [[ -z "$passphrase" ]]; then
        echo "WARNING: KEYSTORE_PASSPHRASE is not set; skipping relayer keystore alignment." >&2
        return 0
    fi

    local keystore_dir="$PROJECT_ROOT/config/oz-relayer/keys"
    mkdir -p "$keystore_dir"

    local signer_name pk_hex signer_addr tmp_dir signer_path

    echo "Aligning OZ relayer keystores with operator keys..."
    for idx in 1 2 3; do
        signer_name="signer-$idx"
        pk_hex="$(get_operator_private_key $((idx - 1)))"
        signer_addr="$(cast wallet address --private-key "$pk_hex" 2>/dev/null || true)"

        tmp_dir="$(mktemp -d)"
        cast wallet import \
            --keystore-dir "$tmp_dir" \
            --private-key "$pk_hex" \
            --unsafe-password "$passphrase" \
            "$signer_name" >/dev/null 2>&1 || {
                rm -rf "$tmp_dir"
                echo "WARNING: Failed to generate keystore for $signer_name" >&2
                continue
            }

        signer_path="$tmp_dir/$signer_name"
        if [[ ! -f "$signer_path" ]]; then
            signer_path="$tmp_dir/$signer_name.json"
        fi
        if [[ ! -f "$signer_path" ]]; then
            rm -rf "$tmp_dir"
            echo "WARNING: Keystore output missing for $signer_name" >&2
            continue
        fi

        mv "$signer_path" "$keystore_dir/$signer_name.json"
        rm -rf "$tmp_dir"

        if [[ -n "$signer_addr" ]]; then
            export "SIGNER_${idx}_ADDRESS=$signer_addr"
        fi
        echo "        ✓ $signer_name aligned${signer_addr:+ ($signer_addr)}"
    done
}

fund_external_signers_if_configured() {
    # On external chains, oz-relayer signers must have native balance to submit txs.
    is_local && return 0

    local deployer_key="${DEPLOYER_PRIVATE_KEY:-$PRIVATE_KEY}"
    local signer_addr amount
    amount="${RELAYER_SIGNER_FUND_AMOUNT:-0.2ether}"

    for idx in 1 2 3; do
        signer_addr="$(printenv "SIGNER_${idx}_ADDRESS" || true)"
        if [[ -z "$signer_addr" || "$signer_addr" == "null" ]]; then
            continue
        fi

        # Skip if this signer already matches operator address for the same slot —
        # operator keys are topped up by ensure_operator_balances() instead.
        local operator_addr
        operator_addr="$(get_operator_address $((idx - 1)) 2>/dev/null || true)"
        if [[ -n "$operator_addr" && "$(printf '%s' "$operator_addr" | tr '[:upper:]' '[:lower:]')" == "$(printf '%s' "$signer_addr" | tr '[:upper:]' '[:lower:]')" ]]; then
            continue
        fi

        cast send "$signer_addr" --value "$amount" \
            --rpc-url "$DEST_RPC" --private-key "$deployer_key" \
            --confirmations 1 >/dev/null 2>&1 || {
                echo "        WARNING: Failed to fund signer-$idx ($signer_addr) with ETH" >&2
                continue
            }
        echo "        ✓ Signer-$idx funded ($signer_addr)"
    done
}

# Top up operator/signer balances on the destination chain if below min_balance.
# Runs on every external startup (including reuse) so relayers never stall on empty wallets.
ensure_operator_balances() {
    is_local && return 0

    local deployer_key="${DEPLOYER_PRIVATE_KEY:-$PRIVATE_KEY}"
    local min_balance="${OPERATOR_MIN_BALANCE:-0.05ether}"
    local top_up="${OPERATOR_TOP_UP_AMOUNT:-0.2ether}"
    local min_wei top_up_wei

    min_wei="$(cast to-wei 0.05 2>/dev/null || echo "50000000000000000")"
    if [[ "$min_balance" != "0.05ether" ]]; then
        min_wei="$(cast --to-wei "${min_balance%ether}" 2>/dev/null || echo "$min_wei")"
    fi

    for i in 0 1 2; do
        local op_addr balance
        op_addr="$(get_operator_address "$i" 2>/dev/null || true)"
        [[ -n "$op_addr" ]] || continue

        balance="$(cast balance "$op_addr" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")"
        if [[ "$balance" -lt "$min_wei" ]] 2>/dev/null; then
            cast send "$op_addr" --value "$top_up" \
                --rpc-url "$DEST_RPC" --private-key "$deployer_key" \
                --confirmations 1 >/dev/null 2>&1 || {
                    echo "        WARNING: Failed to top up operator $i ($op_addr)" >&2
                    continue
                }
            echo "        ✓ Operator $i topped up ($op_addr, was below $min_balance)"
        fi
    done
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

# Relay infra is now tracked in deployments/<env>.json, so there is no
# separate cache file to maintain.
_cache_relay_infra() {
    return 0
}

clear_cached_relay_infra_for_chain() {
    return 0
}

clean_sidecar_runtime_state() {
    local sidecar_dir
    for sidecar_dir in "$PROJECT_ROOT/data/sidecar-1" "$PROJECT_ROOT/data/sidecar-2" "$PROJECT_ROOT/data/sidecar-3"; do
        mkdir -p "$sidecar_dir"
        find "$sidecar_dir" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
    done
    echo "        ✓ Cleared sidecar runtime state"
}

prepare_clean_relay_history() {
    is_local && return 0

    clean_sidecar_runtime_state
}

_cached_relay_infra_has_operator_keys() {
    local relay_json="$1"
    local key_registry
    key_registry="$(jq -r '.keyRegistry // empty' "$relay_json" 2>/dev/null)"
    [[ -n "$key_registry" && "$key_registry" != "null" ]] || {
        echo "        Cached relay infra missing keyRegistry, cannot reuse."
        return 1
    }

    local i op_addr key15 key11
    for i in 0 1 2; do
        op_addr="$(get_operator_address "$i" 2>/dev/null || true)"
        [[ -n "$op_addr" ]] || {
            echo "        Could not derive operator $i address, cannot reuse."
            return 1
        }

        key15="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 15 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        key11="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 11 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        if [[ -z "$key15" || "$key15" == "0x" || -z "$key11" || "$key11" == "0x" ]]; then
            echo "        Operator $i ($op_addr) missing BLS keys (tag15/tag11), cannot reuse existing relay infra."
            return 1
        fi
    done

    return 0
}

maybe_configure_ccv_contracts() {
    local active_provider="$1"
    if [[ "$active_provider" == "chainlink_ccv" ]]; then
        echo "Applying SymbioticCCV remote-chain config..."
        run_make configure-ccv-contracts ENV_CONFIG="$ENV_CONFIG"
    fi
}

wait_for_rpc() {
    local rpc_url="$1"
    local name="$2"
    local timeout=30
    local elapsed=0
    local last_error=""

    if [[ -z "$rpc_url" ]]; then
        echo "      ERROR: No RPC URL configured for ${name}" >&2
        return 1
    fi

    echo "      Connecting to ${name} (${rpc_url})..."
    while true; do
        last_error="$(cast client --rpc-url "$rpc_url" 2>&1)" && break
        sleep 1
        elapsed=$((elapsed + 1))
        if [[ $elapsed -ge $timeout ]]; then
            echo "      ERROR: Timeout waiting for ${name} after ${timeout}s" >&2
            echo "      URL: ${rpc_url}" >&2
            echo "      Last error: ${last_error}" >&2
            return 1
        fi
    done

    echo "      ✓ ${name} ready"
}

run_with_wall_timeout() {
    local timeout_s="$1"
    shift

    "$@" &
    local cmd_pid=$!
    local elapsed=0

    while kill -0 "$cmd_pid" 2>/dev/null; do
        if (( elapsed >= timeout_s )); then
            echo "        WARNING: command timed out after ${timeout_s}s, terminating..." >&2
            kill "$cmd_pid" 2>/dev/null || true
            sleep 1
            kill -9 "$cmd_pid" 2>/dev/null || true
            wait "$cmd_pid" 2>/dev/null || true
            return 124
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    wait "$cmd_pid"
}

deploy_relay_infra_with_retries() {
    local core_config_env="$1"
    local timeout_s="${RELAY_INFRA_WALL_TIMEOUT:-420}"
    local forge_timeout_s="${FORGE_BROADCAST_TIMEOUT:-180}"
    local attempts="${RELAY_INFRA_DEPLOY_ATTEMPTS:-3}"
    local attempt

    # Log the deployment parameters for debugging
    echo "        RPC: $DEST_RPC"
    echo "        Deployer: $(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null || echo "?")"
    local _bal
    _bal="$(cast balance "$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null)" --rpc-url "$DEST_RPC" --ether 2>/dev/null || echo "?")"
    echo "        Balance: ${_bal} ETH"
    echo "        Forge env: $core_config_env"

    for attempt in $(seq 1 "$attempts"); do
        echo "      Deploying relay infra (attempt $attempt/$attempts)..."

        local resume_flag=""
        local gas_multiplier="150"
        if [[ "$attempt" -gt 1 ]]; then
            resume_flag="--resume"
            gas_multiplier="200"
        fi

        local cmd=(
            env $core_config_env forge script script/DeployRelayInfra.s.sol:DeployRelayInfra
            --rpc-url "$DEST_RPC"
            --broadcast
            --private-key "$PRIVATE_KEY"
            --code-size-limit 50000
            --gas-estimate-multiplier "$gas_multiplier"
            --timeout "$forge_timeout_s"
            --slow
            --non-interactive
        )
        if [[ -n "$resume_flag" ]]; then
            cmd+=("$resume_flag")
        fi

        local output
        if output=$(run_with_wall_timeout "$timeout_s" "${cmd[@]}" 2>&1); then
            echo "        ✓ Relay infra deployed"
            return 0
        fi

        # Show the last few lines of output for debugging
        echo "$output" | tail -10 | sed 's/^/        /'

        if [[ "$attempt" -lt "$attempts" ]]; then
            echo "        WARNING: Relay infra deploy attempt $attempt failed; retrying..."
        fi
    done

    echo "ERROR: relay infra deployment failed after $attempts attempts" >&2
    return 1
}

relay_broadcast_created_address() {
    local broadcast_file="$1"
    local contract_name="$2"

    jq -r --arg contract_name "$contract_name" \
        '.transactions[] | select(.contractName == $contract_name and .transactionType == "CREATE") | .contractAddress' \
        "$broadcast_file" | head -n 1
}

refresh_relay_infra_deploy_data_from_broadcast() {
    local dest_chain_id broadcast_file
    dest_chain_id="$(env_chain_id destination)"
    broadcast_file="broadcast/DeployRelayInfra.s.sol/${dest_chain_id}/run-latest.json"

    [[ -f "$broadcast_file" ]] || {
        echo "ERROR: relay infra broadcast not found at $broadcast_file" >&2
        return 1
    }

    local network key_registry voting_powers settlement driver staking_token vault_factory operator_registry network_registry
    network="$(relay_broadcast_created_address "$broadcast_file" "Network")"
    key_registry="$(relay_broadcast_created_address "$broadcast_file" "KeyRegistry")"
    voting_powers="$(relay_broadcast_created_address "$broadcast_file" "VotingPowers")"
    settlement="$(relay_broadcast_created_address "$broadcast_file" "Settlement")"
    driver="$(relay_broadcast_created_address "$broadcast_file" "Driver")"
    staking_token="$(relay_broadcast_created_address "$broadcast_file" "MockERC20")"
    vault_factory="$(relay_broadcast_created_address "$broadcast_file" "VaultFactory")"
    operator_registry="$(relay_broadcast_created_address "$broadcast_file" "OperatorRegistry")"
    network_registry="$(relay_broadcast_created_address "$broadcast_file" "NetworkRegistry")"

    local name value
    for name in network key_registry voting_powers settlement driver staking_token vault_factory operator_registry network_registry; do
        value="${!name}"
        [[ -n "$value" && "$value" != "null" ]] || {
            echo "ERROR: missing $name in $broadcast_file" >&2
            return 1
        }
    done

    jq -n \
        --argjson chainId "$dest_chain_id" \
        --arg network "$network" \
        --arg keyRegistry "$key_registry" \
        --arg votingPowers "$voting_powers" \
        --arg settlement "$settlement" \
        --arg driver "$driver" \
        --arg stakingToken "$staking_token" \
        --arg vaultFactory "$vault_factory" \
        --arg operatorRegistry "$operator_registry" \
        --arg networkRegistry "$network_registry" \
        '{
            chainId: $chainId,
            network: $network,
            keyRegistry: $keyRegistry,
            votingPowers: $votingPowers,
            settlement: $settlement,
            driver: $driver,
            stakingToken: $stakingToken,
            vaultFactory: $vaultFactory,
            operatorRegistry: $operatorRegistry,
            networkRegistry: $networkRegistry
        }' > deploy-data/relay_infra.json
}

deploy_provider_contracts() {
    local active_provider="$1"
    case "$active_provider" in
        layerzero)
            deploy_layerzero_contracts
            ;;
        chainlink_ccv)
            run_make deploy-ccv-contracts ENV_CONFIG="$ENV_CONFIG"
            ;;
        *)
            echo "ERROR: unsupported active_provider '$active_provider'" >&2
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
    source_eid="$(env_eid source)"
    dest_eid="$(env_eid destination)"

    if ! is_local; then
        echo "      Phase 1: Using pre-deployed LayerZero V2 endpoints..."
        local source_chain_id dest_chain_id
        source_chain_id="$(env_chain_id source)"
        dest_chain_id="$(env_chain_id destination)"

        # Generate synthetic layerzero_source.json from env JSON predeploys
        jq -n \
            --argjson chain_id "$source_chain_id" \
            --argjson eid "$source_eid" \
            --arg endpoint "$(env_predeploy source layerzero endpoint)" \
            --arg send_uln "$(env_predeploy source layerzero sendUln302)" \
            '{chainId: $chain_id, eid: $eid, endpoint: $endpoint, sendUln: $send_uln}' \
            > deploy-data/layerzero_source.json
        echo "        ✓ LayerZero source endpoints (pre-deployed)"

        # Generate synthetic layerzero_dest.json from env JSON predeploys
        jq -n \
            --argjson chain_id "$dest_chain_id" \
            --argjson eid "$dest_eid" \
            --arg endpoint "$(env_predeploy destination layerzero endpoint)" \
            --arg receive_uln "$(env_predeploy destination layerzero receiveUln302)" \
            '{chainId: $chain_id, eid: $eid, endpoint: $endpoint, receiveUln: $receive_uln}' \
            > deploy-data/layerzero_dest.json
        echo "        ✓ LayerZero dest endpoints (pre-deployed)"
    fi

    # Pass relay timing from env JSON to Forge scripts (single source of truth).
    local _epoch_dur _slash_win _epoch_delay
    _epoch_dur="$(env_relay epochDurationSeconds)"
    _slash_win="$(env_relay slashingWindowSeconds)"
    _epoch_delay="$(env_relay epochStartDelaySeconds)"
    local relay_env="EPOCH_DURATION=${_epoch_dur} SLASHING_WINDOW=${_slash_win} EPOCH_START_DELAY=${_epoch_delay}"

    # Defensive: EPOCH_START_DELAY=0 will revert on real chains (timestamp drift)
    if ! is_local && [[ "$_epoch_delay" == "0" || -z "$_epoch_delay" ]]; then
        echo "ERROR: relay.epochStartDelaySeconds must be > 0 for external networks (timestamp drift causes revert)" >&2
        exit 1
    fi

    # On external networks, generate Symbiotic Core config from env JSON predeploys
    local core_config_env="$relay_env"
    if ! is_local; then
        local core_config_tmp=".tmp-symbiotic-core-config.json"
        local dest_cid
        dest_cid="$(env_chain_id destination)"
        jq -n --argjson obj "$(env_get '.chains.destination.predeploys.symbioticCore')" \
            --arg chain "$dest_cid" '{($chain): $obj}' > "$core_config_tmp"
        core_config_env="SYMBIOTIC_CORE_CONFIG=$core_config_tmp $relay_env"
    fi

    # Check if relay infra can be reused from a previous deployment
    local relay_infra_reused=0
    if ! is_local && [[ "${FORCE_RELAY_DEPLOY:-0}" != "1" ]]; then
        local existing_settlement
        existing_settlement="$(env_deployment destination relayInfra.settlement 2>/dev/null || true)"

        if [[ -n "$existing_settlement" && "$existing_settlement" != "null" ]]; then
            local code
            code="$(cast code "$existing_settlement" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0x")"

            if [[ "$code" != "0x" && -n "$code" ]]; then
                jq '.destination.relayInfra' "$(deployments_file)" > deploy-data/relay_infra.json
                if _cached_relay_infra_has_operator_keys "deploy-data/relay_infra.json"; then
                    echo "        Reusing existing relay infra (settlement: $existing_settlement)"
                    relay_infra_reused=1
                else
                    echo "        Existing relay infra is incomplete for current operators, deploying fresh..."
                fi
            else
                echo "        Existing relay infra not found on-chain, deploying fresh..."
            fi
        fi
    fi

    if [[ "$relay_infra_reused" == "0" ]]; then
        deploy_relay_infra_with_retries "$core_config_env"
        refresh_relay_infra_deploy_data_from_broadcast

        if ! is_local; then
            echo "      Committing genesis (early — right after relay infra)..."
            # Sync deployments so generate-genesis.sh can read the latest relay infra.
            "$PROJECT_ROOT/scripts/publish-addresses.sh"
            "$PROJECT_ROOT/scripts/generate-genesis.sh"
            echo "        ✓ Genesis committed"

            # Cache relay infra for future reuse
            _cache_relay_infra
        fi
    else
        echo "        ✓ Relay infra reused (skipped deploy + operator registration)"
    fi

    local stack_sig
    if is_local; then
        stack_sig="deployLocal()"
    else
        stack_sig="deployExternal()"
    fi

    echo "      Phase 2: Deploying LayerZero stack..."
    env \
        SOURCE_RPC_URL="$SOURCE_RPC" \
        DEST_RPC_URL="$DEST_RPC" \
        PRIVATE_KEY="$PRIVATE_KEY" \
        LZ_SOURCE_CHAIN_ID="$(env_chain_id source)" \
        LZ_DEST_CHAIN_ID="$(env_chain_id destination)" \
        LZ_SOURCE_EID="$source_eid" \
        LZ_DEST_EID="$dest_eid" \
        OPERATOR_1_PRIVATE_KEY="${OPERATOR_1_PRIVATE_KEY:-}" \
        OPERATOR_2_PRIVATE_KEY="${OPERATOR_2_PRIVATE_KEY:-}" \
        OPERATOR_3_PRIVATE_KEY="${OPERATOR_3_PRIVATE_KEY:-}" \
        forge script script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack \
            --sig "$stack_sig" \
            --broadcast \
            --multi \
            --private-key "$PRIVATE_KEY" \
            --non-interactive \
            $slow_flag \
            --quiet
    echo "        ✓ LayerZero stack deployed"

    )

    # Sync deployed addresses from Forge output into deployments/<env>.json.
    "$PROJECT_ROOT/scripts/publish-addresses.sh"

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
    generate_oz_configs

    maybe_configure_ccv_contracts "$active_provider"
    maybe_refresh_genesis

    if is_local; then
        echo "Refreshing settlement epoch for local devnet..."
        run_make refresh-epoch

        echo "Resetting runtime state for deterministic restart..."
        run_make reset-runtime
    fi

    run_startup_preflight
    run_runtime_validation

    if [[ "$STACK_MODE" == "deploy_only" ]]; then
        echo "Deployment state is valid."
        return 0
    fi

    # Ensure relayer signer wallets have native balance on external chains.
    if ! is_local; then
        echo "Ensuring external signers/operators are funded..."
        ensure_operator_balances
        fund_external_signers_if_configured
    fi

    echo "Starting services..."
    start_provider_services "$active_provider" 1

    echo "Reloading config-driven services (oz-monitor + operators)..."
    docker compose $COMPOSE_FILES --profile dev up -d --force-recreate oz-monitor operator-1 operator-2 operator-3 >/dev/null
    COMPOSE_FILES="$COMPOSE_FILES" ./scripts/start-services.sh "$active_provider" --wait-only >/dev/null
    echo "      ✓ Monitor/operators reloaded"

}

first_run_deploy() {
    local active_provider="$1"

    echo "═══ First run for ${active_provider}: full deployment ═══"
    echo ""

    if is_local; then
        echo "[1/7] Building + starting chains (parallel)..."
        (cd contracts && forge build --quiet && echo "      ✓ Contracts compiled") &
        local build_pid=$!
        (docker compose $COMPOSE_FILES --profile dev build --quiet operator-1 && echo "      ✓ Operator image built" || { echo "      ERROR: Operator image build failed. Is Docker running?" >&2; exit 1; }) &
        local image_pid=$!
        (docker compose $COMPOSE_FILES --profile infra up -d --remove-orphans 2>&1 || { echo "      ERROR: Failed to start infra containers" >&2; exit 1; }; echo "      ✓ Chains starting") &
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
        (docker compose $COMPOSE_FILES --profile dev build --quiet operator-1 && echo "      ✓ Operator image built" || { echo "      ERROR: Operator image build failed. Is Docker running?" >&2; exit 1; }) &
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
    if [[ "$active_provider" == "layerzero" ]] && ! is_local; then
        echo "      ✓ Genesis already committed during deployment"
    else
        "$PROJECT_ROOT/scripts/generate-genesis.sh"
        echo "      ✓ Genesis committed"
    fi

    echo ""
    echo "[5/7] Generating OZ configs..."
    generate_oz_configs

    echo ""
    echo "[6/7] Validating deployment..."
    maybe_configure_ccv_contracts "$active_provider"
    run_startup_preflight
    run_runtime_validation

    if [[ "$STACK_MODE" == "deploy_only" ]]; then
        echo "      ✓ Deployment complete"
        return 0
    fi

    echo ""
    echo "[7/7] Starting services..."
    start_provider_services "$active_provider"
    echo "      ✓ All services started"
}

main() {
    cd "$PROJECT_ROOT"

    mkdir -p "$PROJECT_ROOT/data"
    start_lock_dir="$PROJECT_ROOT/data/.start-stack.lock"
    if ! mkdir "$start_lock_dir" 2>/dev/null; then
        echo "ERROR: another start is already in progress (lock: $start_lock_dir)." >&2
        echo "       If no start is running, remove the lock dir and retry." >&2
        exit 1
    fi
    trap 'rm -rf "$start_lock_dir"' EXIT

    local config_file
    config_file="$(env_config_file)"
    [[ -f "$config_file" ]] || {
        echo "ERROR: missing environment config: $config_file" >&2
        exit 1
    }

    local active_provider
    active_provider="$(get_active_provider)"

    case "$STACK_MODE" in
        full|deploy_only|services_only)
            ;;
        *)
            echo "ERROR: unsupported STACK_MODE '$STACK_MODE'" >&2
            exit 1
            ;;
    esac

    ensure_external_relayer_keystores_match_operator_keys

    if [[ "$STACK_MODE" == "services_only" ]] && ! provider_has_deploy_state "$active_provider"; then
        echo "ERROR: missing deployment state in $(deployments_file). Run 'make deploy ENV=$ENV' first." >&2
        exit 1
    fi

    if [[ "${FORCE_RELAY_DEPLOY:-0}" == "1" ]]; then
        echo "FORCE_RELAY_DEPLOY=1 set: running full deployment path."
        if [[ "${FORCE_CLEAN_RELAY_HISTORY:-1}" == "1" ]]; then
            echo "Preparing clean relay history (cache + sidecar state)..."
            prepare_clean_relay_history
        fi
        first_run_deploy "$active_provider"
    elif provider_has_deploy_state "$active_provider"; then
        resume_existing_deployment "$active_provider"
    else
        first_run_deploy "$active_provider"
    fi

    echo ""
    echo "═══════════════════════════════════════════════════════════════════"
    case "$STACK_MODE" in
        full|services_only)
            echo "Stack started! Run 'make status' to check health."
            ;;
        deploy_only)
            echo "Deployment complete. Run 'make validate ENV=$ENV' or start services."
            ;;
    esac
    echo "═══════════════════════════════════════════════════════════════════"
}

main "$@"
