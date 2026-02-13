#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
DEPLOY_DATA_DIR="${DEPLOY_DATA_DIR:-$PROJECT_ROOT/data/deploy-data}"
DEPLOY_STATE_FILE="${DEPLOY_STATE_FILE:-$DEPLOY_DATA_DIR/deploy-state.json}"
PROVIDER="${1:-${PROVIDER:-}}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi

die() {
    echo "ERROR: $*" >&2
    exit 1
}

require_file() {
    [[ -f "$1" ]] || die "missing file: $1"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"
}

merge_layerzero() {
    require_file "$DEPLOY_DATA_DIR/source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/dest_contracts.json"
    require_file "$DEPLOY_DATA_DIR/layerzero_source.json"
    require_file "$DEPLOY_DATA_DIR/layerzero_dest.json"
    require_file "$DEPLOY_DATA_DIR/testoapp_source.json"
    require_file "$DEPLOY_DATA_DIR/testoapp_dest.json"
    require_file "$DEPLOY_DATA_DIR/relay_infra.json"

    local source_chain_id destination_chain_id source_eid destination_eid
    local source_dvn destination_dvn source_send_uln destination_receive_uln
    local source_endpoint source_executor destination_endpoint
    local source_test_oapp destination_test_oapp destination_settlement
    local relay_destination_json updated_at

    source_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/source_contracts.json")"
    destination_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    source_eid="$(jq -er '.eid | numbers' "$DEPLOY_DATA_DIR/layerzero_source.json")"
    destination_eid="$(jq -er '.eid | numbers' "$DEPLOY_DATA_DIR/layerzero_dest.json")"

    source_dvn="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/source_contracts.json")"
    destination_dvn="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    source_send_uln="$(jq -er '.sendUln' "$DEPLOY_DATA_DIR/source_contracts.json")"
    destination_receive_uln="$(jq -er '.receiveUln' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    source_endpoint="$(jq -er '.endpoint' "$DEPLOY_DATA_DIR/layerzero_source.json")"
    source_executor="$(jq -er '.executor' "$DEPLOY_DATA_DIR/layerzero_source.json")"
    destination_endpoint="$(jq -er '.endpoint' "$DEPLOY_DATA_DIR/layerzero_dest.json")"
    source_test_oapp="$(jq -er '.testOApp' "$DEPLOY_DATA_DIR/testoapp_source.json")"
    destination_test_oapp="$(jq -er '.testOApp' "$DEPLOY_DATA_DIR/testoapp_dest.json")"
    destination_settlement="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    relay_destination_json="$(cat "$DEPLOY_DATA_DIR/relay_infra.json")"
    updated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    local next_state
    next_state="$(mktemp)"

    jq \
        --arg updated_at "$updated_at" \
        --arg source_dvn "$source_dvn" \
        --arg source_send_uln "$source_send_uln" \
        --arg source_endpoint "$source_endpoint" \
        --arg source_executor "$source_executor" \
        --arg source_test_oapp "$source_test_oapp" \
        --arg destination_dvn "$destination_dvn" \
        --arg destination_receive_uln "$destination_receive_uln" \
        --arg destination_endpoint "$destination_endpoint" \
        --arg destination_test_oapp "$destination_test_oapp" \
        --arg destination_settlement "$destination_settlement" \
        --argjson source_chain_id "$source_chain_id" \
        --argjson destination_chain_id "$destination_chain_id" \
        --argjson source_eid "$source_eid" \
        --argjson destination_eid "$destination_eid" \
        --argjson relay_destination "$relay_destination_json" \
        '
        .version = 1 |
        .updated_at = $updated_at |
        .providers = (.providers // {}) |
        .providers.layerzero = {
            source_chain_id: $source_chain_id,
            destination_chain_id: $destination_chain_id,
            source_eid: $source_eid,
            destination_eid: $destination_eid,
            source: {
                dvn: $source_dvn,
                send_uln: $source_send_uln,
                endpoint: $source_endpoint,
                executor: $source_executor,
                test_oapp: $source_test_oapp
            },
            destination: {
                dvn: $destination_dvn,
                receive_uln: $destination_receive_uln,
                endpoint: $destination_endpoint,
                test_oapp: $destination_test_oapp,
                settlement: $destination_settlement
            }
        } |
        .relay_infra = (.relay_infra // {}) |
        .relay_infra.destination = $relay_destination
        ' "$DEPLOY_STATE_FILE" > "$next_state"

    mv "$next_state" "$DEPLOY_STATE_FILE"
}

merge_chainlink_ccv() {
    require_file "$DEPLOY_DATA_DIR/ccv_source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/ccv_dest_contracts.json"

    local source_chain_id destination_chain_id source_selector destination_selector
    local source_ccv destination_ccv source_settlement destination_settlement
    local source_on_ramp source_off_ramp destination_on_ramp destination_off_ramp
    local relay_source_json relay_destination_json updated_at
    local has_relay_source=0 has_relay_destination=0

    source_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    destination_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"

    source_selector="$(jq -r '.providers.chainlink_ccv.source_chain_selector // empty' "$ROOT_CONFIG_FILE" 2>/dev/null || true)"
    destination_selector="$(jq -r '.providers.chainlink_ccv.destination_chain_selector // empty' "$ROOT_CONFIG_FILE" 2>/dev/null || true)"
    [[ -n "$source_selector" ]] || source_selector="$source_chain_id"
    [[ -n "$destination_selector" ]] || destination_selector="$destination_chain_id"
    [[ "$source_selector" =~ ^[0-9]+$ ]] || die "providers.chainlink_ccv.source_chain_selector must be numeric in $ROOT_CONFIG_FILE"
    [[ "$destination_selector" =~ ^[0-9]+$ ]] || die "providers.chainlink_ccv.destination_chain_selector must be numeric in $ROOT_CONFIG_FILE"

    source_ccv="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    destination_ccv="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_settlement="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    destination_settlement="$(jq -er '.settlement' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_on_ramp="$(jq -er '.onRamp' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    source_off_ramp="$(jq -er '.offRamp' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    destination_on_ramp="$(jq -er '.onRamp' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    destination_off_ramp="$(jq -er '.offRamp' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    updated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    if [[ -f "$DEPLOY_DATA_DIR/relay_infra_source.json" ]]; then
        relay_source_json="$(cat "$DEPLOY_DATA_DIR/relay_infra_source.json")"
        has_relay_source=1
    else
        relay_source_json="{}"
    fi

    if [[ -f "$DEPLOY_DATA_DIR/relay_infra.json" ]]; then
        relay_destination_json="$(cat "$DEPLOY_DATA_DIR/relay_infra.json")"
        has_relay_destination=1
    else
        relay_destination_json="{}"
    fi

    local next_state
    next_state="$(mktemp)"

    jq \
        --arg updated_at "$updated_at" \
        --arg source_ccv "$source_ccv" \
        --arg destination_ccv "$destination_ccv" \
        --arg source_settlement "$source_settlement" \
        --arg destination_settlement "$destination_settlement" \
        --arg source_on_ramp "$source_on_ramp" \
        --arg source_off_ramp "$source_off_ramp" \
        --arg destination_on_ramp "$destination_on_ramp" \
        --arg destination_off_ramp "$destination_off_ramp" \
        --argjson source_chain_id "$source_chain_id" \
        --argjson destination_chain_id "$destination_chain_id" \
        --argjson source_selector "$source_selector" \
        --argjson destination_selector "$destination_selector" \
        --argjson relay_source "$relay_source_json" \
        --argjson relay_destination "$relay_destination_json" \
        --argjson has_relay_source "$has_relay_source" \
        --argjson has_relay_destination "$has_relay_destination" \
        '
        .version = 1 |
        .updated_at = $updated_at |
        .providers = (.providers // {}) |
        .providers.chainlink_ccv = {
            source_chain_id: $source_chain_id,
            destination_chain_id: $destination_chain_id,
            source_chain_selector: $source_selector,
            destination_chain_selector: $destination_selector,
            source: {
                ccv: $source_ccv,
                settlement: $source_settlement,
                on_ramp: $source_on_ramp,
                off_ramp: $source_off_ramp
            },
            destination: {
                ccv: $destination_ccv,
                settlement: $destination_settlement,
                on_ramp: $destination_on_ramp,
                off_ramp: $destination_off_ramp
            }
        } |
        .relay_infra = (.relay_infra // {}) |
        (if $has_relay_source == 1 then .relay_infra.source = $relay_source else . end) |
        (if $has_relay_destination == 1 then .relay_infra.destination = $relay_destination else . end)
        ' "$DEPLOY_STATE_FILE" > "$next_state"

    mv "$next_state" "$DEPLOY_STATE_FILE"
}

main() {
    require_cmd jq
    [[ -n "$PROVIDER" ]] || die "usage: $0 <layerzero|chainlink_ccv>"

    mkdir -p "$DEPLOY_DATA_DIR"

    local tmp_state
    tmp_state="$(mktemp)"
    if [[ -f "$DEPLOY_STATE_FILE" ]]; then
        cp "$DEPLOY_STATE_FILE" "$tmp_state"
    else
        printf '%s\n' '{"version":1,"providers":{}}' > "$tmp_state"
    fi

    case "$PROVIDER" in
        layerzero)
            DEPLOY_STATE_FILE="$tmp_state" merge_layerzero
            ;;
        chainlink_ccv)
            DEPLOY_STATE_FILE="$tmp_state" merge_chainlink_ccv
            ;;
        *)
            rm -f "$tmp_state"
            die "unsupported provider: $PROVIDER"
            ;;
    esac

    mv "$tmp_state" "$DEPLOY_STATE_FILE"
    echo "Updated deploy state: $DEPLOY_STATE_FILE (provider: $PROVIDER)"
}

main "$@"
