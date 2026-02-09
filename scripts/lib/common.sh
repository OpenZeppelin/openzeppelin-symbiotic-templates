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
DEPLOY_DATA="$PROJECT_ROOT/data/deploy-data"
ADDRESSES_FILE="$DEPLOY_DATA/addresses.env"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"

# Defaults
SOURCE_RPC="${SOURCE_RPC_URL:-http://localhost:8545}"
DEST_RPC="${DEST_RPC_URL:-http://localhost:8546}"
DEST_EID="${DEST_CHAIN_ID:-31338}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
OPERATOR_PORTS=(3001 3002 3003)

# Load addresses from addresses.env
load_addresses() {
    if [[ -f "$ADDRESSES_FILE" ]]; then
        set -a
        source "$ADDRESSES_FILE"
        set +a
        return 0
    fi
    return 1
}

# Get active provider from root config
get_active_provider() {
    if [[ -f "$ROOT_CONFIG_FILE" ]]; then
        jq -r '.active_provider // "layerzero"' "$ROOT_CONFIG_FILE"
    elif [[ -n "${ACTIVE_PROVIDER:-}" ]]; then
        echo "$ACTIVE_PROVIDER"
    else
        echo "layerzero"
    fi
}

get_ccv_mode() {
    if [[ -f "$ROOT_CONFIG_FILE" ]]; then
        jq -r '.providers.chainlink_ccv.mode // "symbiotic_mock"' "$ROOT_CONFIG_FILE"
    else
        echo "symbiotic_mock"
    fi
}

get_ccv_source_chain_selector() {
    if [[ -n "${CCV_SOURCE_CHAIN_SELECTOR:-}" ]]; then
        echo "$CCV_SOURCE_CHAIN_SELECTOR"
        return 0
    fi

    if [[ -f "$ROOT_CONFIG_FILE" ]]; then
        local configured
        configured="$(jq -r '.providers.chainlink_ccv.source_chain_selector // .providers.chainlink_ccv.source_chain_id // empty' "$ROOT_CONFIG_FILE")"
        if [[ -n "$configured" && "$configured" != "null" ]]; then
            echo "$configured"
            return 0
        fi
    fi

    if [[ -f "$DEPLOY_DATA/ccv_source_contracts.json" ]]; then
        local chain_id
        chain_id="$(jq -r '.chainId // empty' "$DEPLOY_DATA/ccv_source_contracts.json")"
        if [[ -n "$chain_id" && "$chain_id" != "null" ]]; then
            echo "$chain_id"
            return 0
        fi
    fi

    echo "31337"
}

# Get provider configured in generated operator config
get_generated_provider() {
    local generated_config="$PROJECT_ROOT/data/generated-config/operator-1/config.json"
    if [[ -f "$generated_config" ]]; then
        jq -r '.provider // empty' "$generated_config"
    else
        return 1
    fi
}

# Ensure generated runtime configs match root active provider
ensure_provider_alignment() {
    local active_provider="$1"
    local generated_provider

    if generated_provider=$(get_generated_provider 2>/dev/null); then
        if [[ -n "$generated_provider" && "$generated_provider" != "$active_provider" ]]; then
            die "provider mismatch: root config is '$active_provider' but generated operator config is '$generated_provider'. Run 'make configure' and restart operators."
        fi
    fi
}

# Get TestOApp address
get_testoapp_address() {
    if [[ -n "${TEST_OAPP_SOURCE_ADDRESS:-}" ]]; then
        echo "$TEST_OAPP_SOURCE_ADDRESS"
    elif [[ -f "$DEPLOY_DATA/testoapp_source.json" ]]; then
        jq -r '.testOApp' "$DEPLOY_DATA/testoapp_source.json"
    else
        return 1
    fi
}

# Get DVN dest address
get_dvn_dest_address() {
    if [[ -n "${DVN_DEST_ADDRESS:-}" ]]; then
        echo "$DVN_DEST_ADDRESS"
    elif [[ -f "$DEPLOY_DATA/dest_contracts.json" ]]; then
        jq -r '.dvn' "$DEPLOY_DATA/dest_contracts.json"
    else
        return 1
    fi
}

# Get source OnRamp address used for CCV monitor events
get_ccv_source_onramp_address() {
    if [[ -n "${CCV_SOURCE_ONRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_SOURCE_ONRAMP_ADDRESS"
        return 0
    elif [[ -f "$ROOT_CONFIG_FILE" ]]; then
        local configured
        configured=$(jq -r '.providers.chainlink_ccv.source_onramp_address // empty' "$ROOT_CONFIG_FILE")
        if [[ -n "$configured" ]]; then
            echo "$configured"
            return 0
        fi
    fi

    if [[ -f "$DEPLOY_DATA/ccv_source_contracts.json" ]]; then
        jq -r '.onRamp // empty' "$DEPLOY_DATA/ccv_source_contracts.json"
    else
        return 1
    fi
}

get_ccv_dest_offramp_address() {
    if [[ -n "${CCV_DEST_OFFRAMP_ADDRESS:-}" ]]; then
        echo "$CCV_DEST_OFFRAMP_ADDRESS"
        return 0
    elif [[ -f "$ROOT_CONFIG_FILE" ]]; then
        local configured
        configured=$(jq -r '.providers.chainlink_ccv.destination_offramp_address // empty' "$ROOT_CONFIG_FILE")
        if [[ -n "$configured" ]]; then
            echo "$configured"
            return 0
        fi
    fi

    if [[ -f "$DEPLOY_DATA/ccv_dest_contracts.json" ]]; then
        jq -r '.offRamp // empty' "$DEPLOY_DATA/ccv_dest_contracts.json"
    else
        return 1
    fi
}

# Get destination chain selector for CCV messages
get_ccv_dest_chain_selector() {
    if [[ -n "${CCV_DEST_CHAIN_SELECTOR:-}" ]]; then
        echo "$CCV_DEST_CHAIN_SELECTOR"
        return 0
    fi

    if [[ -f "$ROOT_CONFIG_FILE" ]]; then
        local configured
        configured="$(jq -r '.providers.chainlink_ccv.destination_chain_selector // .providers.chainlink_ccv.destination_chain_id // empty' "$ROOT_CONFIG_FILE")"
        if [[ -n "$configured" && "$configured" != "null" ]]; then
            echo "$configured"
            return 0
        fi
    fi

    if [[ -f "$DEPLOY_DATA/ccv_dest_contracts.json" ]]; then
        local chain_id
        chain_id="$(jq -r '.chainId // empty' "$DEPLOY_DATA/ccv_dest_contracts.json")"
        if [[ -n "$chain_id" && "$chain_id" != "null" ]]; then
            echo "$chain_id"
            return 0
        fi
    fi

    echo "31338"
}

get_provider_deploy_marker_file() {
    local provider="$1"
    case "$provider" in
        layerzero)
            echo "$DEPLOY_DATA/relay-infra-complete.marker"
            ;;
        chainlink_ccv)
            echo "$DEPLOY_DATA/ccv-complete.marker"
            ;;
        *)
            return 1
            ;;
    esac
}

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

# Check if DVN verified on dest chain, returns tx hash if found
check_dvn_verified() {
    local dvn_address="$1"
    local from_block="${2:-0}"

    local events
    events=$(cast logs --from-block "$from_block" --address "$dvn_address" --rpc-url "$DEST_RPC" 2>/dev/null | head -1 || true)
    [[ -n "$events" ]]
}

# Get DVN verification tx hash
get_dvn_tx_hash() {
    local dvn_address="$1"
    local from_block="${2:-0}"

    cast logs --from-block "$from_block" --address "$dvn_address" --rpc-url "$DEST_RPC" --json 2>/dev/null | \
        jq -r '.[-1].transactionHash // empty' 2>/dev/null || true
}

# Format operator status for display
format_status() {
    local status=$1
    case $status in
        Pending)    echo "Operators: waiting to batch" ;;
        Processing) echo "Operators: collecting BLS signatures" ;;
        Signed)     echo "Operators: signed (quorum reached)" ;;
        *)          echo "Operators: $status" ;;
    esac
}

# Format relayer submission status for display
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

# Print underlying command (for --dry-run)
print_command() {
    local description="$1"
    shift
    echo "# $description"
    echo "$@"
    echo ""
}

# Die with error message
die() {
    echo "ERROR: $1" >&2
    exit "${2:-1}"
}
