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

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPLOY_DATA_DIR="${DEPLOY_DATA_DIR:-$PROJECT_ROOT/data/deploy-data}"
TEMPLATES_DIR="${TEMPLATES_DIR:-$PROJECT_ROOT/config/templates}"
OUTPUT_DIR="${OUTPUT_DIR:-$PROJECT_ROOT/data/generated-config}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"

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

generate_layerzero_configs() {
    if [[ ! -f "$DEPLOY_DATA_DIR/relay-infra-complete.marker" ]]; then
        echo "ERROR: LayerZero deployment marker missing: $DEPLOY_DATA_DIR/relay-infra-complete.marker" >&2
        exit 1
    fi

    require_file "$DEPLOY_DATA_DIR/source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/dest_contracts.json"
    require_file "$TEMPLATES_DIR/operator/config.json"
    require_file "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json"

    local dvn_src dvn_dst
    dvn_src="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/source_contracts.json")"
    dvn_dst="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/dest_contracts.json")"

    echo "Generating configs for provider: layerzero"
    echo "  DVN Source: $dvn_src"
    echo "  DVN Dest:   $dvn_dst"

    prepare_output_dirs

    for i in 1 2 3; do
        jq --arg dvn "$dvn_dst" \
           --arg relay "http://symbiotic-relay-$i:8080" \
           --arg relayer_id "dvn-relayer-$i" \
            '.provider = "layerzero" |
             .layerzero.dvn_addresses["31338"] = $dvn |
             .oz_relayer.chain_relayers[0].target_address = $dvn |
             .oz_relayer.chain_relayers[0].relayer_id = $relayer_id |
             .symbiotic_relay.address = $relay' \
            "$TEMPLATES_DIR/operator/config.json" > "$OUTPUT_DIR/operator-$i/config.json"

        echo "  Generated: operator-$i/config.json"
    done

    copy_monitor_base

    jq --arg dvn "$dvn_src" '.addresses[0].address = $dvn' \
        "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json" > \
        "$OUTPUT_DIR/oz-monitor/monitors/layerzero_job_assigned.json"
    echo "  Generated: oz-monitor/monitors/layerzero_job_assigned.json"
}

generate_chainlink_ccv_configs() {
    require_file "$DEPLOY_DATA_DIR/ccv_source_contracts.json"
    require_file "$DEPLOY_DATA_DIR/ccv_dest_contracts.json"
    require_file "$TEMPLATES_DIR/operator/config.json"
    require_file "$TEMPLATES_DIR/oz-monitor/monitors/ccip_message_sent.json"

    local ccv_src ccv_dst source_chain_id dest_chain_id source_selector dest_selector ccv_mode
    ccv_src="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    ccv_dst="$(jq -er '.ccv' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_chain_id="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    dest_chain_id="$(jq -er '.chainId' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    source_selector="$(jq -er ".providers.chainlink_ccv.source_chain_selector // $source_chain_id" "$ROOT_CONFIG_FILE")"
    dest_selector="$(jq -er ".providers.chainlink_ccv.destination_chain_selector // $dest_chain_id" "$ROOT_CONFIG_FILE")"
    ccv_mode="$(jq -er '.providers.chainlink_ccv.mode // "symbiotic_mock"' "$ROOT_CONFIG_FILE")"

    case "$ccv_mode" in
        symbiotic_mock)
            ;;
        *)
            echo "ERROR: unsupported providers.chainlink_ccv.mode '$ccv_mode' (expected symbiotic_mock)" >&2
            exit 1
            ;;
    esac

    local source_onramp destination_offramp submit_target
    source_onramp="$(jq -er '.providers.chainlink_ccv.source_onramp_address // empty' "$ROOT_CONFIG_FILE")"
    destination_offramp="$(jq -er '.providers.chainlink_ccv.destination_offramp_address // empty' "$ROOT_CONFIG_FILE")"

    if [[ -z "$source_onramp" ]]; then
        source_onramp="$(jq -er '.onRamp // empty' "$DEPLOY_DATA_DIR/ccv_source_contracts.json")"
    fi
    if [[ -z "$destination_offramp" ]]; then
        destination_offramp="$(jq -er '.offRamp // empty' "$DEPLOY_DATA_DIR/ccv_dest_contracts.json")"
    fi

    if [[ -z "$source_onramp" ]]; then
        echo "ERROR: providers.chainlink_ccv.source_onramp_address is required (or deploy-data/ccv_source_contracts.json.onRamp)" >&2
        exit 1
    fi
    if [[ -z "$destination_offramp" ]]; then
        echo "ERROR: providers.chainlink_ccv.destination_offramp_address is required (or deploy-data/ccv_dest_contracts.json.offRamp)" >&2
        exit 1
    fi

    submit_target="$destination_offramp"

    echo "Generating configs for provider: chainlink_ccv"
    echo "  Mode:        $ccv_mode"
    echo "  Source CCV:  $ccv_src"
    echo "  Dest CCV:    $ccv_dst"
    echo "  Source selector: $source_selector"
    echo "  Dest selector:   $dest_selector"
    echo "  OnRamp:      $source_onramp"
    echo "  Submit to:   $submit_target"

    prepare_output_dirs

    for i in 1 2 3; do
        jq --arg relay "http://symbiotic-relay-$i:8080" \
           --arg relayer_id "dvn-relayer-$i" \
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
            "$TEMPLATES_DIR/operator/config.json" > "$OUTPUT_DIR/operator-$i/config.json"

        echo "  Generated: operator-$i/config.json"
    done

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
