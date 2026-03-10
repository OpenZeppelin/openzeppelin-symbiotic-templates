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

ensure_external_relayer_keystores_match_operator_keys() {
    is_local && return 0

    local passphrase="${KEYSTORE_PASSPHRASE:-}"
    if [[ -z "$passphrase" ]]; then
        echo "WARNING: KEYSTORE_PASSPHRASE is not set; skipping relayer keystore alignment." >&2
        return 0
    fi

    local keystore_dir="$PROJECT_ROOT/config/oz-relayer/keys"
    mkdir -p "$keystore_dir"

    local base_key signer_name pk_hex signer_addr tmp_dir signer_path
    base_key="${OPERATOR_BASE_KEY:-1000000000000000000}"

    echo "Aligning OZ relayer keystores with OPERATOR_BASE_KEY..."
    for idx in 1 2 3; do
        signer_name="signer-$idx"
        pk_hex="$(printf "0x%064x" $((base_key + idx - 1)))"
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

        # Skip if this signer already matches operator address for the same slot.
        local base_key operator_addr
        base_key="${OPERATOR_BASE_KEY:-1000000000000000000}"
        operator_addr="$(cast wallet address --private-key "$(printf "0x%064x" $((base_key + idx - 1)))" 2>/dev/null || true)"
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

clear_cached_relay_infra_for_chain() {
    local chain_id="$1"
    local relay_cache="$PROJECT_ROOT/config/networks/relay-infra.json"
    [[ -f "$relay_cache" ]] || return 0

    local tmp
    tmp="$(mktemp)"
    jq --arg chain "$chain_id" 'del(.[$chain])' "$relay_cache" > "$tmp" && mv "$tmp" "$relay_cache"
    echo "        ✓ Cleared cached relay infra for chain $chain_id"
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

    local dest_chain_id
    dest_chain_id="$(jq -er '.providers.layerzero.destination_chain_id | numbers' "$ROOT_CONFIG_FILE" 2>/dev/null || true)"
    if [[ -n "$dest_chain_id" ]]; then
        clear_cached_relay_infra_for_chain "$dest_chain_id"
    fi

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

    local base_key
    base_key="${OPERATOR_BASE_KEY:-1000000000000000000}"

    local i op_addr key15 key11
    for i in 0 1 2; do
        op_addr="$(cast wallet address --private-key "$(printf "0x%064x" $((base_key + i)))" 2>/dev/null || true)"
        [[ -n "$op_addr" ]] || {
            echo "        Could not derive operator $i address from OPERATOR_BASE_KEY, cannot reuse."
            return 1
        }

        key15="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 15 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        key11="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 11 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        if [[ -z "$key15" || "$key15" == "0x" || -z "$key11" || "$key11" == "0x" ]]; then
            echo "        Operator $i ($op_addr) missing BLS keys (tag15/tag11), cannot reuse cached relay infra."
            return 1
        fi
    done

    return 0
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
            --quiet
        )
        if [[ -n "$resume_flag" ]]; then
            cmd+=("$resume_flag")
        fi

        if run_with_wall_timeout "$timeout_s" "${cmd[@]}"; then
            echo "        ✓ Relay infra deployed"
            return 0
        fi

        if [[ "$attempt" -lt "$attempts" ]]; then
            echo "        WARNING: Relay infra deploy attempt $attempt failed; retrying..."
        fi
    done

    echo "ERROR: relay infra deployment failed after $attempts attempts" >&2
    return 1
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
                    # Restore relay_infra.json from cache, then validate key health for current OPERATOR_BASE_KEY.
                    jq --arg chain "$dest_chain_id" '.[$chain]' "$relay_cache" > deploy-data/relay_infra.json
                    if _cached_relay_infra_has_operator_keys "deploy-data/relay_infra.json"; then
                        echo "        Reusing existing relay infra on chain $dest_chain_id (settlement: $cached_settlement)"
                        relay_infra_reused=1
                    else
                        echo "        Cached relay infra is incomplete for current operators, deploying fresh..."
                    fi
                else
                    echo "        Cached relay infra not found on-chain, deploying fresh..."
                fi
            fi
        fi
    fi

    if [[ "$relay_infra_reused" == "0" ]]; then
        deploy_relay_infra_with_retries "$core_config_env"

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
                cast send "$op_addr" --value "${OPERATOR_FUND_AMOUNT:-0.2ether}" \
                    --rpc-url "$DEST_RPC" --private-key "$PRIVATE_KEY" \
                    --confirmations 1 >/dev/null 2>&1 || {
                        echo "        WARNING: Failed to fund operator $i with ETH (may already be funded)" >&2
                    }
                # Transfer staking tokens
                cast send "$staking_token" "transfer(address,uint256)" "$op_addr" "100000000000000000000000" \
                    --rpc-url "$DEST_RPC" --private-key "$PRIVATE_KEY" \
                    --confirmations 1 >/dev/null 2>&1 || {
                        echo "        WARNING: Failed to transfer staking tokens to operator $i (may already have tokens)" >&2
                    }
                echo "        ✓ Operator $i funded ($op_addr)"
            done

            # Fund explicit oz-relayer signer addresses as well (if configured).
            # This prevents relayer health failures when signers differ from operator keys.
            fund_external_signers_if_configured

            # Phase 2: Register all operators in one call (minimizes epoch gap)
            env $core_config_env forge script script/RegisterOperators.s.sol:RegisterOperators \
                --sig "registerAllOperators()" \
                --rpc-url "$DEST_RPC" \
                --broadcast \
                --private-key "$PRIVATE_KEY" \
                --slow \
                --quiet
            echo "        ✓ All operators registered"

            # Commit genesis IMMEDIATELY after operator registration to minimize
            # the epoch gap.  Epochs before key registration have no BLS keys and
            # would cause sidecar sync failures if we wait too long.
            echo "      Committing genesis (early — right after operator registration)..."
            # Genesis script reads from data/deploy-data/, so copy relay_infra there
            mkdir -p "$PROJECT_ROOT/data/deploy-data"
            cp deploy-data/relay_infra.json "$PROJECT_ROOT/data/deploy-data/relay_infra.json"
            ROOT_CONFIG_FILE="$ROOT_CONFIG_FILE" "$PROJECT_ROOT/scripts/generate-genesis.sh"
            echo "        ✓ Genesis committed"

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

    # Ensure relayer signer wallets have native balance on external chains.
    if ! is_local; then
        echo "Ensuring external relayer signers are funded..."
        fund_external_signers_if_configured
    fi

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

    mkdir -p "$PROJECT_ROOT/data"
    start_lock_dir="$PROJECT_ROOT/data/.start-stack.lock"
    if ! mkdir "$start_lock_dir" 2>/dev/null; then
        echo "ERROR: another start is already in progress (lock: $start_lock_dir)." >&2
        echo "       If no start is running, remove the lock dir and retry." >&2
        exit 1
    fi
    trap 'rm -rf "$start_lock_dir"' EXIT

    local active_provider
    [[ -f "$ROOT_CONFIG_FILE" ]] || {
        echo "ERROR: missing root config: $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    active_provider="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE" 2>/dev/null)" || {
        echo "ERROR: invalid root config: expected .active_provider in $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    ensure_external_relayer_keystores_match_operator_keys

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
    echo "Stack started! Run 'make status' to check health."
    echo "═══════════════════════════════════════════════════════════════════"
}

main "$@"
