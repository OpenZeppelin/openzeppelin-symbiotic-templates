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

# Check dependencies
require() { command -v "$1" >/dev/null 2>&1 || { echo "ERROR: missing dependency: $1" >&2; exit 1; }; }
require jq

# Check deployment is complete (use relay-infra marker as it's the last one created)
if [[ ! -f "$DEPLOY_DATA_DIR/relay-infra-complete.marker" ]]; then
    echo "ERROR: Contracts not deployed. Run 'make start' first." >&2
    exit 1
fi

# Check required files
if [[ ! -f "$DEPLOY_DATA_DIR/source_contracts.json" ]] || [[ ! -f "$DEPLOY_DATA_DIR/dest_contracts.json" ]]; then
    echo "ERROR: Missing deployment data files." >&2
    exit 1
fi

# Extract DVN addresses (use -e to fail on missing keys)
DVN_SRC="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/source_contracts.json")" || {
    echo "ERROR: Missing .dvn in source_contracts.json" >&2
    exit 1
}
DVN_DST="$(jq -er '.dvn' "$DEPLOY_DATA_DIR/dest_contracts.json")" || {
    echo "ERROR: Missing .dvn in dest_contracts.json" >&2
    exit 1
}

echo "Generating configs..."
echo "  DVN Source: $DVN_SRC"
echo "  DVN Dest:   $DVN_DST"

# Clean and create output directories
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR/operator-1"
mkdir -p "$OUTPUT_DIR/operator-2"
mkdir -p "$OUTPUT_DIR/operator-3"
mkdir -p "$OUTPUT_DIR/oz-monitor/monitors"
mkdir -p "$OUTPUT_DIR/oz-monitor/networks"
mkdir -p "$OUTPUT_DIR/oz-monitor/triggers"

# Generate operator configs (patch DVN address, relay URL, and relayer ID)
for i in 1 2 3; do
    if [[ -f "$TEMPLATES_DIR/operator/config.json" ]]; then
        TEMPLATE="$TEMPLATES_DIR/operator/config.json"
    else
        echo "ERROR: Template not found: $TEMPLATES_DIR/operator/config.json" >&2
        exit 1
    fi

    # Patch the DVN address, symbiotic-relay URL, and relayer ID per operator
    jq --arg dvn "$DVN_DST" \
       --arg relay "http://symbiotic-relay-$i:8080" \
       --arg relayer_id "dvn-relayer-$i" \
        '.layerzero.dvn_addresses["31338"] = $dvn |
         .oz_relayer.chain_relayers[0].dvn_address = $dvn |
         .oz_relayer.chain_relayers[0].relayer_id = $relayer_id |
         .symbiotic_relay.address = $relay' \
        "$TEMPLATE" > "$OUTPUT_DIR/operator-$i/config.json"

    echo "  Generated: operator-$i/config.json"
done

# Generate oz-monitor configs (copy all, patch monitor with source DVN address)
# Copy network and trigger configs as-is
if [[ -d "$TEMPLATES_DIR/oz-monitor/networks" ]]; then
    cp "$TEMPLATES_DIR/oz-monitor/networks/"* "$OUTPUT_DIR/oz-monitor/networks/" 2>/dev/null || true
fi
if [[ -d "$TEMPLATES_DIR/oz-monitor/triggers" ]]; then
    cp "$TEMPLATES_DIR/oz-monitor/triggers/"* "$OUTPUT_DIR/oz-monitor/triggers/" 2>/dev/null || true
fi

# Patch monitor with source DVN address
if [[ -f "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json" ]]; then
    jq --arg dvn "$DVN_SRC" '.addresses[0].address = $dvn' \
        "$TEMPLATES_DIR/oz-monitor/monitors/layerzero_job_assigned.json" > \
        "$OUTPUT_DIR/oz-monitor/monitors/layerzero_job_assigned.json"
    echo "  Generated: oz-monitor/monitors/layerzero_job_assigned.json"
fi

echo "Config generation complete."
