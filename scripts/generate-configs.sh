#!/usr/bin/env bash
# Generate runtime configs from templates
#
# This script:
# 1. Reads template configs from config/templates/
# 2. Patches them with deployed contract addresses
# 3. Writes to data/generated-config/
#
# Usage: ./scripts/generate-configs.sh
#        make configure

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DEPLOY_DATA_DIR="${DEPLOY_DATA_DIR:-$PROJECT_ROOT/data/deploy-data}"
TEMPLATES_DIR="${TEMPLATES_DIR:-$PROJECT_ROOT/config/templates}"
OUTPUT_DIR="${OUTPUT_DIR:-$PROJECT_ROOT/data/generated-config}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
DEPLOY_DATA="$DEPLOY_DATA_DIR"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

# Check dependencies
require() { command -v "$1" >/dev/null 2>&1 || { echo "ERROR: missing dependency: $1" >&2; exit 1; }; }
require jq

if [[ ! -f "$ROOT_CONFIG_FILE" ]]; then
    echo "ERROR: Missing root config: $ROOT_CONFIG_FILE" >&2
    exit 1
fi

require_file() {
    [[ -f "$1" ]] || {
        echo "ERROR: missing file: $1" >&2
        exit 1
    }
}

prepare_output_dirs() {
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR/operator-1"
    mkdir -p "$OUTPUT_DIR/operator-2"
    mkdir -p "$OUTPUT_DIR/operator-3"
    mkdir -p "$OUTPUT_DIR/oz-monitor/monitors"
    mkdir -p "$OUTPUT_DIR/oz-monitor/networks"
    mkdir -p "$OUTPUT_DIR/oz-monitor/triggers"
}

copy_monitor_base() {
    if [[ -d "$TEMPLATES_DIR/oz-monitor/networks" ]]; then
        cp "$TEMPLATES_DIR/oz-monitor/networks/"* "$OUTPUT_DIR/oz-monitor/networks/" 2>/dev/null || true
    fi
    if [[ -d "$TEMPLATES_DIR/oz-monitor/triggers" ]]; then
        cp "$TEMPLATES_DIR/oz-monitor/triggers/"* "$OUTPUT_DIR/oz-monitor/triggers/" 2>/dev/null || true
    fi
}

render_layerzero_operator_config() {
    local operator_index="$1"
    local dvn_address="$2"
    local source_chain_id="$3"
    local dest_chain_id="$4"
    local source_eid="$5"
    local dest_eid="$6"

    jq --arg dvn "$dvn_address" \
       --arg relay "http://symbiotic-relay-${operator_index}:8080" \
       --arg relayer_id "dvn-relayer-${operator_index}" \
       --argjson source_chain_id "$source_chain_id" \
       --argjson dest_chain_id "$dest_chain_id" \
       --arg source_eid "$source_eid" \
       --arg dest_eid "$dest_eid" \
        '.provider = "layerzero" |
         .database.path = "/app/data/layerzero/redb" |
         .destination_chains = [$dest_chain_id] |
         .layerzero.eid_to_chain_id = {
           ($source_eid): $source_chain_id,
           ($dest_eid): $dest_chain_id
         } |
         .layerzero.dvn_addresses = {
           ($dest_chain_id | tostring): $dvn
         } |
         .oz_relayer.chain_relayers[0].chain_id = $dest_chain_id |
         .oz_relayer.chain_relayers[0].target_address = $dvn |
         .oz_relayer.chain_relayers[0].relayer_id = $relayer_id |
         .symbiotic_relay.address = $relay' \
        "$TEMPLATES_DIR/operator/config.json"
}

render_chainlink_ccv_operator_config() {
    local operator_index="$1"
    local submit_target="$2"
    local ccv_src="$3"
    local ccv_dst="$4"
    local source_onramp="$5"
    local destination_offramp="$6"
    local source_chain_id="$7"
    local dest_chain_id="$8"
    local source_selector="$9"
    local dest_selector="${10}"

    jq --arg relay "http://symbiotic-relay-${operator_index}:8080" \
       --arg relayer_id "dvn-relayer-${operator_index}" \
       --arg submit_target "$submit_target" \
       --arg ccv_src "$ccv_src" \
       --arg ccv_dst "$ccv_dst" \
       --arg source_onramp "$source_onramp" \
       --arg destination_offramp "$destination_offramp" \
       --argjson source_chain_id "$source_chain_id" \
       --argjson dest_chain_id "$dest_chain_id" \
       --argjson source_selector "$source_selector" \
       --argjson dest_selector "$dest_selector" \
        '.provider = "chainlink_ccv" |
         .database.path = "/app/data/chainlink_ccv/redb" |
         .destination_chains = [$dest_chain_id] |
         .symbiotic_relay.address = $relay |
         .oz_relayer.chain_relayers[0].chain_id = $dest_chain_id |
         .oz_relayer.chain_relayers[0].relayer_id = $relayer_id |
         .oz_relayer.chain_relayers[0].target_address = $submit_target |
         .layerzero = null |
         .chainlink_ccv = {
           source_chain_id: $source_chain_id,
           destination_chain_id: $dest_chain_id,
           source_chain_selector: $source_selector,
           destination_chain_selector: $dest_selector,
           source_ccv_address: $ccv_src,
           destination_ccv_address: $ccv_dst,
           source_onramp_address: $source_onramp,
           destination_offramp_address: $destination_offramp
         }' \
        "$TEMPLATES_DIR/operator/config.json"
}

generate_operator_configs() {
    local renderer="$1"
    shift

    for operator_index in 1 2 3; do
        "$renderer" "$operator_index" "$@" > "$OUTPUT_DIR/operator-${operator_index}/config.json"
        echo "  Generated: operator-${operator_index}/config.json"
    done
}

generate_layerzero_configs() {
    if [[ ! -f "$DEPLOY_DATA_DIR/relay-infra-complete.marker" ]]; then
        echo "ERROR: LayerZero deployment marker missing: $DEPLOY_DATA_DIR/relay-infra-complete.marker" >&2
        exit 1
    fi

    require_file "$DEPLOY_DATA_DIR/source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/dest_contracts.json"
    require_file "$DEPLOY_DATA_DIR/layerzero_source.json"
    require_file "$DEPLOY_DATA_DIR/layerzero_dest.json"
    require_file "$TEMPLATES_DIR/operator/config.json"
    require_file "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json"

    local dvn_src dvn_dst
    local root_source_chain_id root_dest_chain_id root_source_eid root_dest_eid
    local deploy_source_chain_id deploy_dest_chain_id deploy_source_eid deploy_dest_eid
    dvn_src="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/source_contracts.json")"
    dvn_dst="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    root_source_chain_id="$(jq -er '.providers.layerzero.source_chain_id | numbers' "$ROOT_CONFIG_FILE")" || {
        echo "ERROR: providers.layerzero.source_chain_id must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }
    root_dest_chain_id="$(jq -er '.providers.layerzero.destination_chain_id | numbers' "$ROOT_CONFIG_FILE")" || {
        echo "ERROR: providers.layerzero.destination_chain_id must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }
    root_source_eid="$(jq -er '.providers.layerzero.source_eid | numbers' "$ROOT_CONFIG_FILE")" || {
        echo "ERROR: providers.layerzero.source_eid must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }
    root_dest_eid="$(jq -er '.providers.layerzero.destination_eid | numbers' "$ROOT_CONFIG_FILE")" || {
        echo "ERROR: providers.layerzero.destination_eid must be numeric in $ROOT_CONFIG_FILE" >&2
        exit 1
    }

    deploy_source_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/source_contracts.json")"
    deploy_dest_chain_id="$(jq -er '.chainId | numbers' "$DEPLOY_DATA_DIR/dest_contracts.json")"
    deploy_source_eid="$(jq -er '.eid | numbers' "$DEPLOY_DATA_DIR/layerzero_source.json")"
    deploy_dest_eid="$(jq -er '.eid | numbers' "$DEPLOY_DATA_DIR/layerzero_dest.json")"

    [[ "$root_source_chain_id" == "$deploy_source_chain_id" ]] || {
        echo "ERROR: providers.layerzero.source_chain_id ($root_source_chain_id) does not match deploy-data/source_contracts.json.chainId ($deploy_source_chain_id)" >&2
        exit 1
    }
    [[ "$root_dest_chain_id" == "$deploy_dest_chain_id" ]] || {
        echo "ERROR: providers.layerzero.destination_chain_id ($root_dest_chain_id) does not match deploy-data/dest_contracts.json.chainId ($deploy_dest_chain_id)" >&2
        exit 1
    }
    [[ "$root_source_eid" == "$deploy_source_eid" ]] || {
        echo "ERROR: providers.layerzero.source_eid ($root_source_eid) does not match deploy-data/layerzero_source.json.eid ($deploy_source_eid)" >&2
        exit 1
    }
    [[ "$root_dest_eid" == "$deploy_dest_eid" ]] || {
        echo "ERROR: providers.layerzero.destination_eid ($root_dest_eid) does not match deploy-data/layerzero_dest.json.eid ($deploy_dest_eid)" >&2
        exit 1
    }

    echo "Generating configs for provider: layerzero"
    echo "  Source chain/EID: ${root_source_chain_id}/${root_source_eid}"
    echo "  Dest chain/EID:   ${root_dest_chain_id}/${root_dest_eid}"
    echo "  DVN Source:       $dvn_src"
    echo "  DVN Dest:         $dvn_dst"

    prepare_output_dirs

    generate_operator_configs \
        render_layerzero_operator_config \
        "$dvn_dst" \
        "$root_source_chain_id" \
        "$root_dest_chain_id" \
        "$root_source_eid" \
        "$root_dest_eid"

    copy_monitor_base

    jq --arg dvn "$dvn_src" '.addresses[0].address = $dvn' \
        "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json" > \
        "$OUTPUT_DIR/oz-monitor/monitors/layerzero_job_assigned.json"
    echo "  Generated: oz-monitor/monitors/layerzero_job_assigned.json"
}

generate_chainlink_ccv_configs() {
    if [[ ! -f "$DEPLOY_DATA_DIR/ccv-complete.marker" ]]; then
        echo "ERROR: Chainlink CCV deployment marker missing: $DEPLOY_DATA_DIR/ccv-complete.marker" >&2
        exit 1
    fi
    if [[ ! -f "$DEPLOY_DATA_DIR/relay-infra-complete.marker" ]]; then
        echo "ERROR: Relay infrastructure marker missing: $DEPLOY_DATA_DIR/relay-infra-complete.marker" >&2
        exit 1
    fi

    require_file "$DEPLOY_DATA_DIR/ccv_source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/ccv_dest_contracts.json"
    require_file "$TEMPLATES_DIR/operator/config.json"
    require_file "$TEMPLATES_DIR/oz-monitor/monitors/ccip_message_sent.json"

    local ccv_src ccv_dst source_chain_id dest_chain_id source_selector dest_selector
    ccv_src="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    ccv_dst="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_chain_id="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    dest_chain_id="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_selector="$(get_ccv_source_chain_selector)"
    dest_selector="$(get_ccv_dest_chain_selector)"

    local source_onramp source_offramp destination_onramp destination_offramp submit_target
    source_onramp="$(get_ccv_source_onramp_address 2>/dev/null || true)"
    source_offramp="$(get_ccv_source_offramp_address 2>/dev/null || true)"
    destination_onramp="$(get_ccv_dest_onramp_address 2>/dev/null || true)"
    destination_offramp="$(get_ccv_dest_offramp_address 2>/dev/null || true)"

    if [[ -z "$source_onramp" ]]; then
        echo "ERROR: missing CCV source onRamp address (set CCV_SOURCE_ONRAMP_ADDRESS or deploy CCV contracts)" >&2
        exit 1
    fi
    if [[ -z "$source_offramp" ]]; then
        echo "ERROR: missing CCV source offRamp address (set CCV_SOURCE_OFFRAMP_ADDRESS or deploy CCV contracts)" >&2
        exit 1
    fi
    if [[ -z "$destination_onramp" ]]; then
        echo "ERROR: missing CCV destination onRamp address (set CCV_DEST_ONRAMP_ADDRESS or deploy CCV contracts)" >&2
        exit 1
    fi
    if [[ -z "$destination_offramp" ]]; then
        echo "ERROR: missing CCV destination offRamp address (set CCV_DEST_OFFRAMP_ADDRESS or deploy CCV contracts)" >&2
        exit 1
    fi

    submit_target="$destination_offramp"

    echo "Generating configs for provider: chainlink_ccv"
    echo "  Source CCV:  $ccv_src"
    echo "  Dest CCV:    $ccv_dst"
    echo "  Source selector: $source_selector"
    echo "  Dest selector:   $dest_selector"
    echo "  Source OnRamp: $source_onramp"
    echo "  Source OffRamp: $source_offramp"
    echo "  Dest OnRamp:   $destination_onramp"
    echo "  Submit to:     $submit_target"

    prepare_output_dirs

    generate_operator_configs \
        render_chainlink_ccv_operator_config \
        "$submit_target" \
        "$ccv_src" \
        "$ccv_dst" \
        "$source_onramp" \
        "$destination_offramp" \
        "$source_chain_id" \
        "$dest_chain_id" \
        "$source_selector" \
        "$dest_selector"

    copy_monitor_base

    jq --arg onramp "$source_onramp" '.addresses[0].address = $onramp' \
        "$TEMPLATES_DIR/oz-monitor/monitors/ccip_message_sent.json" > \
        "$OUTPUT_DIR/oz-monitor/monitors/ccip_message_sent.json"
    echo "  Generated: oz-monitor/monitors/ccip_message_sent.json"
}

active_provider="$(jq -er '.active_provider' "$ROOT_CONFIG_FILE")"
case "$active_provider" in
    layerzero)
        generate_layerzero_configs
        ;;
    chainlink_ccv)
        generate_chainlink_ccv_configs
        ;;
    *)
        echo "ERROR: Unsupported active_provider '$active_provider' in $ROOT_CONFIG_FILE" >&2
        exit 1
        ;;
esac

echo "Config generation complete."
