#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

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

require_env_deployments() {
    local provider="$1"

    env_has_deployments source || die "no source deployments in $(deployments_file). Run 'make deploy'."
    env_has_deployments destination || die "no destination deployments in $(deployments_file). Run 'make deploy'."

    case "$provider" in
        layerzero)
            local src_dvn dst_dvn src_oapp dst_oapp
            src_dvn="$(env_deployment source dvn)"
            dst_dvn="$(env_deployment destination dvn)"
            src_oapp="$(env_deployment source testOApp)"
            dst_oapp="$(env_deployment destination testOApp)"
            [[ -n "$src_dvn" && "$src_dvn" != "null" ]] || die "missing source DVN deployment in $(deployments_file)"
            [[ -n "$dst_dvn" && "$dst_dvn" != "null" ]] || die "missing destination DVN deployment in $(deployments_file)"
            [[ -n "$src_oapp" && "$src_oapp" != "null" ]] || die "missing source TestOApp deployment in $(deployments_file)"
            [[ -n "$dst_oapp" && "$dst_oapp" != "null" ]] || die "missing destination TestOApp deployment in $(deployments_file)"
            ;;
        chainlink_ccv)
            local src_ccv dst_ccv
            src_ccv="$(env_deployment source chainlinkCcv.ccv)"
            dst_ccv="$(env_deployment destination chainlinkCcv.ccv)"
            [[ -n "$src_ccv" && "$src_ccv" != "null" ]] || die "missing source CCV deployment in $(deployments_file)"
            [[ -n "$dst_ccv" && "$dst_ccv" != "null" ]] || die "missing destination CCV deployment in $(deployments_file)"
            ;;
        *)
            die "unsupported provider '$provider'"
            ;;
    esac
}

validate_external_network() {
    # Validate env vars are set
    [[ -n "${SOURCE_RPC:-}" ]] || die "SOURCE RPC is required for non-local deployments"
    [[ -n "${DEST_RPC:-}" ]] || die "DEST RPC is required for non-local deployments"
    [[ -n "${PRIVATE_KEY:-}" ]] || die "PRIVATE_KEY is required for non-local deployments"

    # Warn if using default anvil key on external network
    if [[ "${PRIVATE_KEY:-}" == "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" ]]; then
        die "PRIVATE_KEY is set to the default Anvil key -- this will not work on external networks"
    fi

    # Verify RPCs are reachable
    cast client --rpc-url "$SOURCE_RPC" >/dev/null 2>&1 || \
        die "cannot reach source RPC: $SOURCE_RPC"
    cast client --rpc-url "$DEST_RPC" >/dev/null 2>&1 || \
        die "cannot reach destination RPC: $DEST_RPC"

    # Verify chain IDs match environment config
    local expected_source expected_dest actual_source actual_dest
    expected_source="$(env_chain_id source)"
    expected_dest="$(env_chain_id destination)"

    actual_source="$(cast chain-id --rpc-url "$SOURCE_RPC" 2>/dev/null)" || \
        die "failed to get chain ID from source RPC"
    actual_dest="$(cast chain-id --rpc-url "$DEST_RPC" 2>/dev/null)" || \
        die "failed to get chain ID from destination RPC"

    [[ "$actual_source" == "$expected_source" ]] || \
        die "source chain ID mismatch: RPC reports $actual_source, config expects $expected_source"
    [[ "$actual_dest" == "$expected_dest" ]] || \
        die "destination chain ID mismatch: RPC reports $actual_dest, config expects $expected_dest"

    # Verify deployer has non-zero balance
    local deployer_address balance
    deployer_address="$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null)" || \
        die "invalid PRIVATE_KEY"

    balance="$(cast balance "$deployer_address" --rpc-url "$SOURCE_RPC" 2>/dev/null)" || true
    [[ -n "$balance" && "$balance" != "0" ]] || \
        die "deployer $deployer_address has zero balance on source chain ($SOURCE_RPC)"

    balance="$(cast balance "$deployer_address" --rpc-url "$DEST_RPC" 2>/dev/null)" || true
    [[ -n "$balance" && "$balance" != "0" ]] || \
        die "deployer $deployer_address has zero balance on destination chain ($DEST_RPC)"

    echo "External network validation passed (deployer: $deployer_address)"
}

main() {
    local config_file
    config_file="$(env_config_file)"
    require_file "$config_file"

    local active_provider monitor_file
    active_provider="$(get_active_provider)"

    # External network validation
    if ! is_local; then
        validate_external_network
    fi

    require_env_deployments "$active_provider"

    if [[ "$active_provider" == "layerzero" ]]; then
        monitor_file="$GENERATED_DIR/oz-monitor/monitors/layerzero_job_assigned.json"
        require_file "$monitor_file"
    else
        local src_selector dst_selector src_onramp src_offramp dst_onramp dst_offramp

        src_selector="$(get_ccv_source_chain_selector)"
        dst_selector="$(get_ccv_dest_chain_selector)"
        [[ "$src_selector" =~ ^[0-9]+$ ]] || die "invalid source chain selector: '$src_selector'"
        [[ "$dst_selector" =~ ^[0-9]+$ ]] || die "invalid destination chain selector: '$dst_selector'"

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

        monitor_file="$GENERATED_DIR/oz-monitor/monitors/ccip_message_sent.json"
        require_file "$monitor_file"
    fi

    echo "Preflight checks passed for provider: $active_provider"
}

main "$@"
