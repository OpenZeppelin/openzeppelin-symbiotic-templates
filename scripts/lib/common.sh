#!/usr/bin/env bash
# Shared functions for devnet testing scripts
#
# Usage: source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

# Get project root (parent of scripts directory)
# common.sh is at scripts/lib/common.sh, so go up two levels
get_project_root() {
    cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}

# Paths
PROJECT_ROOT="${PROJECT_ROOT:-$(get_project_root)}"
CACHE_DIR="$PROJECT_ROOT/.cache"
CACHE_FILE="$CACHE_DIR/last-message.json"

# ── Environment config integration ───────────────────────────────────────────
# Source env-config.sh for reading the environment JSON.
source "$(dirname "${BASH_SOURCE[0]}")/env-config.sh"
export ENV_CONFIG="${ENV_CONFIG:-$PROJECT_ROOT/config/environments/${ENV:-local}.json}"
export GENERATED_DIR="${GENERATED_DIR:-$PROJECT_ROOT/generated/${ENV:-local}}"

# ── Core detection ────────────────────────────────────────────────────────────

# Detect local mode (anvil chain ID = 31337)
is_local() {
    env_is_local
}

# Defaults -- local anvil always uses localhost regardless of .env RPC vars.
if is_local; then
    SOURCE_RPC="http://localhost:8545"
    DEST_RPC="http://localhost:8546"
    PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
else
    SOURCE_RPC="${SOURCE_RPC_URL:-}"
    DEST_RPC="${DEST_RPC_URL:-}"
    PRIVATE_KEY="${PRIVATE_KEY:-}"
fi
DEST_EID="${DEST_CHAIN_ID:-$(env_eid destination)}"
OPERATOR_PORTS=(3001 3002 3003)

# ── Operator key management ───────────────────────────────────────────────────

# Get the private key for operator N (0-based index).
get_operator_private_key() {
    local index="$1"  # 0-based
    local op_num=$((index + 1))
    local env_var="OPERATOR_${op_num}_PRIVATE_KEY"
    local key="${!env_var:-}"
    if [[ -z "$key" ]]; then
        echo "ERROR: ${env_var} is not set. Run 'make setup' to generate operator keys." >&2
        return 1
    fi
    echo "$key"
}

# Get the operator EVM address for operator N (0-based index).
get_operator_address() {
    local index="$1"
    local pk
    pk="$(get_operator_private_key "$index")"
    cast wallet address --private-key "$pk" 2>/dev/null
}

# ── Provider and deployment state ─────────────────────────────────────────────

# Get active provider from environment config
get_active_provider() {
    local provider
    provider="$(env_active_provider)"
    if [[ -z "$provider" || "$provider" == "null" ]]; then
        die "missing .activeProvider in $(env_config_file)"
    fi
    echo "$provider"
}

# Check if deployments are populated for the given provider
provider_has_deploy_state() {
    local provider="$1"
    env_has_deployments source && env_has_deployments destination
}

# ── LayerZero address getters ─────────────────────────────────────────────────

get_layerzero_source_eid() {
    if [[ -n "${LZ_SOURCE_EID:-}" ]]; then
        echo "$LZ_SOURCE_EID"
    else
        env_eid source
    fi
}

get_layerzero_dest_eid() {
    if [[ -n "${LZ_DEST_EID:-}" ]]; then
        echo "$LZ_DEST_EID"
    else
        env_eid destination
    fi
}

# Get TestOApp address on source chain
get_testoapp_address() {
    if [[ -n "${TEST_OAPP_SOURCE_ADDRESS:-}" ]]; then
        echo "$TEST_OAPP_SOURCE_ADDRESS"
    else
        env_deployment source testOApp
    fi
}

# Get LayerZero DVN address on destination chain
get_layerzero_dest_target_address() {
    if [[ -n "${DVN_DEST_ADDRESS:-}" ]]; then
        echo "$DVN_DEST_ADDRESS"
    else
        env_deployment destination dvn
    fi
}

# ── Chainlink CCV getters ────────────────────────────────────────────────────

get_ccv_source_chain_selector() {
    if [[ -n "${CCV_SOURCE_CHAIN_SELECTOR:-}" ]]; then
        echo "$CCV_SOURCE_CHAIN_SELECTOR"
    else
        env_chain_id source
    fi
}

get_ccv_dest_chain_selector() {
    if [[ -n "${CCV_DEST_CHAIN_SELECTOR:-}" ]]; then
        echo "$CCV_DEST_CHAIN_SELECTOR"
    else
        env_chain_id destination
    fi
}

get_ccv_source_address() {
    if [[ -n "${CCV_SOURCE_ADDRESS:-}" ]]; then
        echo "$CCV_SOURCE_ADDRESS"
    else
        env_deployment source chainlinkCcv.ccv
    fi
}

get_ccv_dest_address() {
    if [[ -n "${CCV_DEST_ADDRESS:-}" ]]; then
        echo "$CCV_DEST_ADDRESS"
    else
        env_deployment destination chainlinkCcv.ccv
    fi
}

get_ccv_source_onramp_address() {
    if [[ -n "${CCV_SOURCE_ONRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_SOURCE_ONRAMP_ADDRESS"
    else
        env_deployment source chainlinkCcv.onRamp
    fi
}

get_ccv_dest_offramp_address() {
    if [[ -n "${CCV_DEST_OFFRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_DEST_OFFRAMP_ADDRESS"
    else
        env_deployment destination chainlinkCcv.offRamp
    fi
}

get_ccv_source_offramp_address() {
    if [[ -n "${CCV_SOURCE_OFFRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_SOURCE_OFFRAMP_ADDRESS"
    else
        env_deployment source chainlinkCcv.offRamp
    fi
}

get_ccv_dest_onramp_address() {
    if [[ -n "${CCV_DEST_ONRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_DEST_ONRAMP_ADDRESS"
    else
        env_deployment destination chainlinkCcv.onRamp
    fi
}

# ── Message cache and operator query ──────────────────────────────────────────

# Load cached message data
load_cached_message() {
    if [[ -f "$CACHE_FILE" ]]; then
        cat "$CACHE_FILE"
    else
        echo "{}"
    fi
}

# Save message to cache
save_to_cache() {
    local tx_hash="$1"
    local block="$2"
    local guid="$3"
    local message="$4"
    local dest_eid="$5"

    mkdir -p "$CACHE_DIR"

    local guid_json
    if [[ -n "$guid" && "$guid" != "null" ]]; then
        guid_json="\"$guid\""
    else
        guid_json="null"
    fi

    cat > "$CACHE_FILE" <<EOF
{
  "tx_hash": "$tx_hash",
  "block": $block,
  "guid": $guid_json,
  "message": "$message",
  "dest_eid": $dest_eid,
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
}

# Query operator for message status
# Args: port, guid (optional), tx_hash (optional)
query_operator() {
    local port=$1
    local guid="${2:-}"
    local tx_hash="${3:-}"

    local response
    response=$(curl -sf "http://localhost:$port/debug/v1/messages?limit=50" 2>/dev/null || echo "{}")

    if [[ "$response" == "{}" ]]; then
        echo "{}"
        return
    fi

    if [[ -n "$guid" && "$guid" != "null" ]]; then
        echo "$response" | jq --arg id "$guid" '.messages[]? | select(.metadata.message_id == $id)' 2>/dev/null || echo "{}"
    elif [[ -n "$tx_hash" ]]; then
        echo "$response" | jq --arg tx "$tx_hash" '.messages[]? | select(.metadata.event_tx_hash == $tx)' 2>/dev/null || echo "{}"
    else
        echo "$response" | jq '.messages[0]? // {}' 2>/dev/null || echo "{}"
    fi
}

# Find GUID from operators by TX hash
find_guid_by_tx() {
    local tx_hash="$1"

    for port in "${OPERATOR_PORTS[@]}"; do
        local response
        response=$(curl -sf "http://localhost:$port/debug/v1/messages?limit=10" 2>/dev/null || echo "{}")
        if [[ "$response" != "{}" ]]; then
            local guid
            guid=$(echo "$response" | jq -r --arg tx "$tx_hash" \
                '.messages[]? | select(.metadata.event_tx_hash == $tx) | .metadata.message_id' 2>/dev/null | head -1)
            if [[ -n "$guid" && "$guid" != "null" ]]; then
                echo "$guid"
                return 0
            fi
        fi
    done
    return 1
}

# Check whether LayerZero target emitted a verification event on destination chain
check_layerzero_target_verified() {
    local target_address="$1"
    local from_block="${2:-0}"

    local events
    events=$(cast logs --from-block "$from_block" --address "$target_address" --rpc-url "$DEST_RPC" 2>/dev/null | head -1 || true)
    [[ -n "$events" ]]
}

# Get LayerZero target verification tx hash
get_layerzero_target_tx_hash() {
    local target_address="$1"
    local from_block="${2:-0}"

    cast logs --from-block "$from_block" --address "$target_address" --rpc-url "$DEST_RPC" --json 2>/dev/null | \
        jq -r '.[-1].transactionHash // empty' 2>/dev/null || true
}

# ── Display formatting ────────────────────────────────────────────────────────

format_status() {
    local status=$1
    case $status in
        Pending)    echo "Operators: waiting to batch" ;;
        Processing) echo "Operators: collecting BLS signatures" ;;
        Signed)     echo "Operators: signed (quorum reached)" ;;
        *)          echo "Operators: $status" ;;
    esac
}

format_relayer_status() {
    local state=$1
    local tx_hash=$2
    case $state in
        Pending)    echo "Relayer: queued" ;;
        Submitted)  echo "Relayer: submitted" ;;
        Confirmed)
            if [[ -n "$tx_hash" ]]; then
                echo "Relayer: confirmed (tx: $tx_hash)"
            else
                echo "Relayer: confirmed"
            fi
            ;;
        Failed)     echo "Relayer: failed" ;;
        *)          echo "Relayer: $state" ;;
    esac
}

print_command() {
    local description="$1"
    shift
    echo "# $description"
    echo "$@"
    echo ""
}

die() {
    echo "ERROR: $1" >&2
    exit "${2:-1}"
}

# ── OZ config generation ────────────────────────────────────────────────────
# Generate OZ Monitor + OZ Relayer configs from the environment JSON.
# OZ services are upstream images with fixed config formats — they can't read
# our env JSON directly, so we generate their configs here.
#
# Usage: generate_oz_configs [output_dir]
generate_oz_configs() {
    local output_dir="${1:-$GENERATED_DIR}"
    local templates="$PROJECT_ROOT/config/templates"
    local provider
    provider="$(env_active_provider)"

    mkdir -p "$output_dir/oz-monitor/networks" \
             "$output_dir/oz-monitor/monitors" \
             "$output_dir/oz-monitor/triggers" \
             "$output_dir/oz-relayer/networks"

    # ── Monitor: network definition ──
    if env_is_local; then
        cp "$templates/oz-monitor/networks/local_anvil.json" \
           "$output_dir/oz-monitor/networks/local_anvil.json"
    else
        local src_chain_id slug
        src_chain_id="$(env_chain_id source)"
        slug="chain_${src_chain_id}"
        jq -n \
            --arg slug "$slug" \
            --arg name "Chain $src_chain_id" \
            --argjson chain_id "$src_chain_id" \
            --arg rpc_url "${SOURCE_RPC:?SOURCE RPC required for non-local}" \
            --argjson block_time "$(env_get '.chains.source.blockTimeMs')" \
            --argjson confirms "$(env_get '.chains.source.confirmations')" \
            --arg cron "$(env_get '.ozMonitor.cronSchedule')" \
            --argjson max_past "$(env_get '.ozMonitor.maxPastBlocks')" \
            '{
                slug: $slug, name: $name, network_type: "EVM", chain_id: $chain_id,
                rpc_urls: [{type_: "rpc", url: {type: "plain", value: $rpc_url}, weight: 100}],
                block_time_ms: $block_time, confirmation_blocks: $confirms,
                cron_schedule: $cron, max_past_blocks: $max_past, store_blocks: false
            }' > "$output_dir/oz-monitor/networks/${slug}.json"
    fi

    # ── Monitor: triggers (static, same for all providers) ──
    cp "$templates/oz-monitor/triggers/"* "$output_dir/oz-monitor/triggers/" 2>/dev/null || true

    # ── Monitor: job definition ──
    local monitor_address template_file
    case "$provider" in
        layerzero)
            monitor_address="$(env_deployment source dvn)"
            template_file="$templates/oz-monitor/monitors/layerzero_job_assigned.json"
            ;;
        chainlink_ccv)
            monitor_address="$(get_ccv_source_onramp_address)"
            template_file="$templates/oz-monitor/monitors/ccip_message_sent.json"
            ;;
        *)  die "generate_oz_configs: unsupported provider '$provider'" ;;
    esac

    if env_is_local; then
        jq --arg addr "$monitor_address" \
            '.addresses[0].address = $addr' \
            "$template_file" > "$output_dir/oz-monitor/monitors/$(basename "$template_file")"
    else
        jq --arg addr "$monitor_address" --arg net "chain_$(env_chain_id source)" \
            '.addresses[0].address = $addr | .networks = [$net]' \
            "$template_file" > "$output_dir/oz-monitor/monitors/$(basename "$template_file")"
    fi

    # ── Relayer: config + network definition ──
    local static_relayer="$PROJECT_ROOT/config/oz-relayer/config.json"
    if env_is_local; then
        cp "$static_relayer" "$output_dir/oz-relayer/config.json"
    else
        local dst_chain_id net_name
        dst_chain_id="$(env_chain_id destination)"
        net_name="chain-${dst_chain_id}"

        jq -n \
            --argjson chain_id "$dst_chain_id" \
            --arg rpc_url "${DEST_RPC:?DEST RPC required for non-local}" \
            --arg net "$net_name" \
            --argjson block_time "$(env_get '.chains.destination.blockTimeMs')" \
            --argjson confirms "$(env_get '.chains.destination.confirmations')" \
            '{
                networks: [{
                    type: "evm", network: $net, chain_id: $chain_id,
                    required_confirmations: $confirms, symbol: "ETH",
                    rpc_urls: [$rpc_url], explorer_urls: [],
                    average_blocktime_ms: $block_time,
                    is_testnet: true, features: ["eip1559"]
                }]
            }' > "$output_dir/oz-relayer/networks/dest-network.json"

        jq --arg net "$net_name" --arg bal "$(env_get '.ozRelayer.minBalanceWei')" \
            '.relayers = [.relayers[] | .network = $net | .policies.min_balance = ($bal | tonumber)]' \
            "$static_relayer" > "$output_dir/oz-relayer/config.json"
    fi

    echo "OZ configs generated in $output_dir"
}
