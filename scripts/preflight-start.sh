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

main() {
    require_file "$ROOT_CONFIG_FILE"

    local active_provider monitor_file
    active_provider="$(jq -r '.active_provider // "layerzero"' "$ROOT_CONFIG_FILE")"
    case "$active_provider" in
        layerzero|chainlink_ccv)
            ;;
        *)
            die "unsupported active_provider '$active_provider' in $ROOT_CONFIG_FILE"
            ;;
    esac

    require_generated_provider_config "$active_provider"

    if [[ "$active_provider" == "layerzero" ]]; then
        monitor_file="$PROJECT_ROOT/data/generated-config/oz-monitor/monitors/layerzero_job_assigned.json"
        require_file "$monitor_file"
        require_file "$PROJECT_ROOT/data/deploy-data/deployment-complete.marker"
        require_file "$PROJECT_ROOT/data/deploy-data/relay-infra-complete.marker"
    else
        local ccv_mode src_selector dst_selector onramp offramp
        ccv_mode="$(get_ccv_mode)"
        [[ "$ccv_mode" == "symbiotic_mock" ]] || die "unsupported providers.chainlink_ccv.mode '$ccv_mode' (expected symbiotic_mock)"

        src_selector="$(get_ccv_source_chain_selector)"
        dst_selector="$(get_ccv_dest_chain_selector)"
        [[ "$src_selector" =~ ^[0-9]+$ ]] || die "invalid providers.chainlink_ccv.source_chain_selector: '$src_selector'"
        [[ "$dst_selector" =~ ^[0-9]+$ ]] || die "invalid providers.chainlink_ccv.destination_chain_selector: '$dst_selector'"

        onramp="$(get_ccv_source_onramp_address 2>/dev/null || true)"
        offramp="$(get_ccv_dest_offramp_address 2>/dev/null || true)"
        [[ -n "$onramp" ]] || die "missing CCV source onRamp. Set providers.chainlink_ccv.source_onramp_address or deploy CCV contracts."
        [[ -n "$offramp" ]] || die "missing CCV destination offRamp. Set providers.chainlink_ccv.destination_offramp_address or deploy CCV contracts."
        is_hex_address "$onramp" || die "invalid CCV source onRamp address: $onramp"
        is_hex_address "$offramp" || die "invalid CCV destination offRamp address: $offramp"

        monitor_file="$PROJECT_ROOT/data/generated-config/oz-monitor/monitors/ccip_message_sent.json"
        require_file "$monitor_file"
        require_file "$PROJECT_ROOT/data/deploy-data/ccv-complete.marker"
        require_file "$PROJECT_ROOT/data/deploy-data/relay-infra-complete.marker"
    fi

    echo "Preflight checks passed for provider: $active_provider"
}

main "$@"
