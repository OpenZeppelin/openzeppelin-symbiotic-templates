#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-config/root.config.json}"
DEPLOY_DATA="${DEPLOY_DATA:-$PROJECT_ROOT/data/deploy-data}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi

die() {
    echo "ERROR: $*" >&2
    exit 1
}

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

require_file() {
    local file="$1"
    [[ -f "$file" ]] || die "required file missing: $file"
}

is_hex_address() {
    [[ "${1:-}" =~ ^0x[0-9a-fA-F]{40}$ ]]
}

require_generated_provider_config() {
    local expected_provider="$1"
    local file actual_provider

    for idx in 1 2 3; do
        file="$PROJECT_ROOT/data/generated-config/operator-${idx}/config.json"
        require_file "$file"
        actual_provider="$(jq -r '.provider // empty' "$file")"
        [[ "$actual_provider" == "$expected_provider" ]] || die "operator-${idx} config provider mismatch: expected '$expected_provider', got '${actual_provider:-<empty>}' ($file). Run 'make configure'."
    done
}

require_provider_state_basics() {
    local provider="$1"
    require_file "$DEPLOY_STATE_FILE"

    case "$provider" in
        layerzero)
            jq -e '
                .providers.layerzero as $lz |
                ($lz.source_chain_id | type == "number") and
                ($lz.destination_chain_id | type == "number") and
                ($lz.source_eid | type == "number") and
                ($lz.destination_eid | type == "number") and
                ($lz.source.dvn | type == "string" and test("^0x[0-9a-fA-F]{40}$")) and
                ($lz.destination.dvn | type == "string" and test("^0x[0-9a-fA-F]{40}$")) and
                ($lz.source.test_oapp | type == "string" and test("^0x[0-9a-fA-F]{40}$")) and
                ($lz.destination.test_oapp | type == "string" and test("^0x[0-9a-fA-F]{40}$"))
            ' "$DEPLOY_STATE_FILE" >/dev/null 2>&1 || \
                die "deploy state incomplete for provider '$provider' (expected $DEPLOY_STATE_FILE). Run 'make start'."
            ;;
        chainlink_ccv)
            jq -e '
                .providers.chainlink_ccv as $ccv |
                ($ccv.source_chain_id | type == "number") and
                ($ccv.destination_chain_id | type == "number") and
                ($ccv.source_chain_selector | type == "number") and
                ($ccv.destination_chain_selector | type == "number") and
                ($ccv.source.ccv | type == "string" and test("^0x[0-9a-fA-F]{40}$")) and
                ($ccv.destination.ccv | type == "string" and test("^0x[0-9a-fA-F]{40}$"))
            ' "$DEPLOY_STATE_FILE" >/dev/null 2>&1 || \
                die "deploy state incomplete for provider '$provider' (expected $DEPLOY_STATE_FILE). Run 'make start'."
            ;;
        *)
            die "unsupported active_provider '$provider' in $ROOT_CONFIG_FILE"
            ;;
    esac
}

validate_external_network() {
    # Validate env vars are set
    [[ -n "${SOURCE_RPC_URL:-}" ]] || die "SOURCE_RPC_URL is required for non-local deployments"
    [[ -n "${DEST_RPC_URL:-}" ]] || die "DEST_RPC_URL is required for non-local deployments"
    [[ -n "${PRIVATE_KEY:-}" ]] || die "PRIVATE_KEY is required for non-local deployments"

    # Warn if using default anvil key on external network
    if [[ "${PRIVATE_KEY:-}" == "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" ]]; then
        die "PRIVATE_KEY is set to the default Anvil key -- this will not work on external networks"
    fi

    # Verify RPCs are reachable
    cast client --rpc-url "$SOURCE_RPC_URL" >/dev/null 2>&1 || \
        die "cannot reach source RPC: $SOURCE_RPC_URL"
    cast client --rpc-url "$DEST_RPC_URL" >/dev/null 2>&1 || \
        die "cannot reach destination RPC: $DEST_RPC_URL"

    # Verify chain IDs match root config
    local expected_source expected_dest actual_source actual_dest
    expected_source="$(jq -r '.providers[.active_provider].source_chain_id // .providers[.active_provider].source_chain_selector // empty' "$ROOT_CONFIG_FILE")"
    expected_dest="$(jq -r '.providers[.active_provider].destination_chain_id // .providers[.active_provider].destination_chain_selector // empty' "$ROOT_CONFIG_FILE")"

    actual_source="$(cast chain-id --rpc-url "$SOURCE_RPC_URL" 2>/dev/null)" || \
        die "failed to get chain ID from source RPC"
    actual_dest="$(cast chain-id --rpc-url "$DEST_RPC_URL" 2>/dev/null)" || \
        die "failed to get chain ID from destination RPC"

    [[ "$actual_source" == "$expected_source" ]] || \
        die "source chain ID mismatch: RPC reports $actual_source, config expects $expected_source"
    [[ "$actual_dest" == "$expected_dest" ]] || \
        die "destination chain ID mismatch: RPC reports $actual_dest, config expects $expected_dest"

    # Verify deployer has non-zero balance
    local deployer_address balance
    deployer_address="$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null)" || \
        die "invalid PRIVATE_KEY"

    balance="$(cast balance "$deployer_address" --rpc-url "$SOURCE_RPC_URL" 2>/dev/null)" || true
    [[ -n "$balance" && "$balance" != "0" ]] || \
        die "deployer $deployer_address has zero balance on source chain ($SOURCE_RPC_URL)"

    balance="$(cast balance "$deployer_address" --rpc-url "$DEST_RPC_URL" 2>/dev/null)" || true
    [[ -n "$balance" && "$balance" != "0" ]] || \
        die "deployer $deployer_address has zero balance on destination chain ($DEST_RPC_URL)"

    echo "External network validation passed (deployer: $deployer_address)"
}

main() {
    require_file "$ROOT_CONFIG_FILE"

    local active_provider monitor_file
    active_provider="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE" 2>/dev/null)" || \
        die "invalid root config: expected .active_provider in $ROOT_CONFIG_FILE"
    case "$active_provider" in
        layerzero|chainlink_ccv)
            ;;
        *)
            die "unsupported active_provider '$active_provider' in $ROOT_CONFIG_FILE"
            ;;
    esac

    # External network validation
    if ! is_local; then
        validate_external_network
    fi

    require_generated_provider_config "$active_provider"
    require_provider_state_basics "$active_provider"

    if [[ "$active_provider" == "layerzero" ]]; then
        monitor_file="$PROJECT_ROOT/data/generated-config/oz-monitor/monitors/layerzero_job_assigned.json"
        require_file "$monitor_file"
    else
        local src_selector dst_selector src_onramp src_offramp dst_onramp dst_offramp

        src_selector="$(get_ccv_source_chain_selector)"
        dst_selector="$(get_ccv_dest_chain_selector)"
        [[ "$src_selector" =~ ^[0-9]+$ ]] || die "invalid providers.chainlink_ccv.source_chain_selector: '$src_selector'"
        [[ "$dst_selector" =~ ^[0-9]+$ ]] || die "invalid providers.chainlink_ccv.destination_chain_selector: '$dst_selector'"

        src_onramp="$(get_ccv_source_onramp_address 2>/dev/null || true)"
        src_offramp="$(get_ccv_source_offramp_address 2>/dev/null || true)"
        dst_onramp="$(get_ccv_dest_onramp_address 2>/dev/null || true)"
        dst_offramp="$(get_ccv_dest_offramp_address 2>/dev/null || true)"
        [[ -n "$src_onramp" ]] || die "missing CCV source onRamp. Set CCV_SOURCE_ONRAMP_ADDRESS or deploy CCV contracts."
        [[ -n "$src_offramp" ]] || die "missing CCV source offRamp. Set CCV_SOURCE_OFFRAMP_ADDRESS or deploy CCV contracts."
        [[ -n "$dst_onramp" ]] || die "missing CCV destination onRamp. Set CCV_DEST_ONRAMP_ADDRESS or deploy CCV contracts."
        [[ -n "$dst_offramp" ]] || die "missing CCV destination offRamp. Set CCV_DEST_OFFRAMP_ADDRESS or deploy CCV contracts."
        is_hex_address "$src_onramp" || die "invalid CCV source onRamp address: $src_onramp"
        is_hex_address "$src_offramp" || die "invalid CCV source offRamp address: $src_offramp"
        is_hex_address "$dst_onramp" || die "invalid CCV destination onRamp address: $dst_onramp"
        is_hex_address "$dst_offramp" || die "invalid CCV destination offRamp address: $dst_offramp"

        monitor_file="$PROJECT_ROOT/data/generated-config/oz-monitor/monitors/ccip_message_sent.json"
        require_file "$monitor_file"
    fi

    echo "Preflight checks passed for provider: $active_provider"
}

main "$@"
