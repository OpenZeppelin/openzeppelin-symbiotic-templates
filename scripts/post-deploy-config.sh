#!/bin/bash
# DEPRECATED: This script is superseded by:
#   - scripts/generate-configs.sh  (generates runtime configs from templates)
#   - scripts/generate-addresses.sh (generates addresses.env)
#
# Use 'make configure' instead, which calls both scripts.
#
# This script is kept for backwards compatibility but may be removed in a future version.
#
# ---
# Original description:
# Post-deployment configuration script
# Reads contract addresses from deploy-state.json and updates operator config
#
# Usage:
#   ./scripts/post-deploy-config.sh                    # Standalone mode
#   docker run ... scripts/post-deploy-config.sh      # Docker mode
#
# Environment variables:
#   DEPLOY_DATA_DIR   - Directory containing deploy-state.json (default: /deploy-data)
#   CONFIG_FILE       - Sidecar config file to update (default: /config/config.yaml)
#   MARKER_TIMEOUT    - Seconds to wait for deploy state (default: 300)

set -euo pipefail

# Configuration with defaults
DEPLOY_DATA_DIR="${DEPLOY_DATA_DIR:-/deploy-data}"
CONFIG_FILE="${CONFIG_FILE:-/config/config.yaml}"
MARKER_TIMEOUT="${MARKER_TIMEOUT:-300}"
MARKER_FILE="${DEPLOY_DATA_DIR}/deploy-state.json"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Wait for deployment to complete
wait_for_deployment() {
    log_info "Waiting for deploy state (timeout: ${MARKER_TIMEOUT}s)..."

    local elapsed=0
    while [ ! -f "$MARKER_FILE" ]; do
        if [ $elapsed -ge $MARKER_TIMEOUT ]; then
            log_error "Timeout waiting for deploy state file: $MARKER_FILE"
            exit 1
        fi
        sleep 2
        elapsed=$((elapsed + 2))
        if [ $((elapsed % 10)) -eq 0 ]; then
            log_info "Still waiting... (${elapsed}s elapsed)"
        fi
    done

    log_info "Deploy state found!"
}

# Extract address from JSON file using portable methods
extract_address() {
    local file="$1"
    local key="$2"

    if [ ! -f "$file" ]; then
        log_error "File not found: $file"
        return 1
    fi

    # Try jq first, fall back to grep/sed
    if command -v jq &> /dev/null; then
        jq -r ".$key" "$file"
    else
        # Portable fallback using grep and sed
        grep -o "\"$key\"[[:space:]]*:[[:space:]]*\"0x[a-fA-F0-9]*\"" "$file" | \
            sed 's/.*"0x/0x/' | sed 's/".*//'
    fi
}

# Read all contract addresses
read_addresses() {
    log_info "Reading contract addresses from deploy state..."

    if ! command -v jq &> /dev/null; then
        log_error "jq is required to read deploy state: $MARKER_FILE"
        exit 1
    fi

    if jq -e '.providers.layerzero != null' "$MARKER_FILE" >/dev/null 2>&1; then
        DVN_SOURCE_ADDRESS="$(jq -er '.providers.layerzero.source.dvn' "$MARKER_FILE")"
        DVN_DEST_ADDRESS="$(jq -er '.providers.layerzero.destination.dvn' "$MARKER_FILE")"
        SEND_ULN_ADDRESS="$(jq -er '.providers.layerzero.source.send_uln' "$MARKER_FILE")"
        RECEIVE_ULN_ADDRESS="$(jq -er '.providers.layerzero.destination.receive_uln' "$MARKER_FILE")"
        SETTLEMENT_ADDRESS="$(jq -er '.providers.layerzero.destination.settlement // .relay_infra.destination.settlement' "$MARKER_FILE")"
        SOURCE_CHAIN_ID="$(jq -er '.providers.layerzero.source_chain_id | numbers' "$MARKER_FILE")"
        DEST_CHAIN_ID="$(jq -er '.providers.layerzero.destination_chain_id | numbers' "$MARKER_FILE")"

        log_info "Source DVN: $DVN_SOURCE_ADDRESS"
        log_info "SendUln: $SEND_ULN_ADDRESS"
        log_info "Source Chain ID: $SOURCE_CHAIN_ID"
        log_info "Dest DVN: $DVN_DEST_ADDRESS"
        log_info "ReceiveUln: $RECEIVE_ULN_ADDRESS"
        log_info "Settlement: $SETTLEMENT_ADDRESS"
        log_info "Dest Chain ID: $DEST_CHAIN_ID"
    elif jq -e '.providers.chainlink_ccv != null' "$MARKER_FILE" >/dev/null 2>&1; then
        log_warn "LayerZero deploy state missing; using chainlink_ccv addresses"

        DVN_SOURCE_ADDRESS="$(jq -er '.providers.chainlink_ccv.source.ccv' "$MARKER_FILE")"
        DVN_DEST_ADDRESS="$(jq -er '.providers.chainlink_ccv.destination.ccv' "$MARKER_FILE")"
        SEND_ULN_ADDRESS="$(jq -er '.providers.chainlink_ccv.source.on_ramp' "$MARKER_FILE")"
        RECEIVE_ULN_ADDRESS="$(jq -er '.providers.chainlink_ccv.destination.off_ramp' "$MARKER_FILE")"
        SETTLEMENT_ADDRESS="$(jq -er '.providers.chainlink_ccv.destination.settlement // .relay_infra.destination.settlement' "$MARKER_FILE")"
        SOURCE_CHAIN_ID="$(jq -er '.providers.chainlink_ccv.source_chain_id | numbers' "$MARKER_FILE")"
        DEST_CHAIN_ID="$(jq -er '.providers.chainlink_ccv.destination_chain_id | numbers' "$MARKER_FILE")"

        log_info "Source target: $DVN_SOURCE_ADDRESS"
        log_info "Source entrypoint: $SEND_ULN_ADDRESS"
        log_info "Source Chain ID: $SOURCE_CHAIN_ID"
        log_info "Dest target: $DVN_DEST_ADDRESS"
        log_info "Dest entrypoint: $RECEIVE_ULN_ADDRESS"
        log_info "Settlement: $SETTLEMENT_ADDRESS"
        log_info "Dest Chain ID: $DEST_CHAIN_ID"
    else
        log_error "No supported provider section found in deploy state: $MARKER_FILE"
        exit 1
    fi

    # Export for use by other scripts
    export DVN_SOURCE_ADDRESS
    export DVN_DEST_ADDRESS
    export SEND_ULN_ADDRESS
    export RECEIVE_ULN_ADDRESS
    export SETTLEMENT_ADDRESS
    export SOURCE_CHAIN_ID
    export DEST_CHAIN_ID
}

# Update operator config file with actual addresses
update_config() {
    log_info "Updating operator config: $CONFIG_FILE"

    if [ ! -f "$CONFIG_FILE" ]; then
        log_warn "Config file not found: $CONFIG_FILE - skipping update"
        return 0
    fi

    # Create a temporary file for the updated config
    local temp_file=$(mktemp)

    # Replace placeholders with actual addresses
    sed -e "s|\${DVN_SOURCE_ADDRESS}|${DVN_SOURCE_ADDRESS}|g" \
        -e "s|\${DVN_DEST_ADDRESS}|${DVN_DEST_ADDRESS}|g" \
        -e "s|\${SEND_ULN_ADDRESS}|${SEND_ULN_ADDRESS}|g" \
        -e "s|\${RECEIVE_ULN_ADDRESS}|${RECEIVE_ULN_ADDRESS}|g" \
        -e "s|\${SETTLEMENT_ADDRESS}|${SETTLEMENT_ADDRESS}|g" \
        -e "s|\${SOURCE_CHAIN_ID}|${SOURCE_CHAIN_ID}|g" \
        -e "s|\${DEST_CHAIN_ID}|${DEST_CHAIN_ID}|g" \
        "$CONFIG_FILE" > "$temp_file"

    # Replace original file (use cat > to handle mounted files)
    cat "$temp_file" > "$CONFIG_FILE"
    rm -f "$temp_file"

    log_info "Config file updated successfully"
}

# Update oz-monitor config with deployed DVN address
update_oz_monitor_config() {
    local oz_monitor_dir="${OZ_MONITOR_CONFIG_DIR:-}"
    local monitor_file="${oz_monitor_dir}/layerzero_job_assigned.json"

    if [ -z "$oz_monitor_dir" ]; then
        log_info "OZ_MONITOR_CONFIG_DIR not set, skipping oz-monitor update"
        return 0
    fi

    if [ ! -f "$monitor_file" ]; then
        log_warn "oz-monitor config not found: $monitor_file - skipping update"
        return 0
    fi

    log_info "Updating oz-monitor config with DVN address: $DVN_SOURCE_ADDRESS"

    # Create a temporary file
    local temp_file=$(mktemp)

    # Update the DVN address in the monitor config using jq if available, else sed
    if command -v jq &> /dev/null; then
        jq --arg dvn "$DVN_SOURCE_ADDRESS" \
           '.addresses[0].address = $dvn' \
           "$monitor_file" > "$temp_file"
    else
        # Fallback: use sed to replace the address (matches 0x followed by 40 hex chars)
        sed -E "s/\"address\":[[:space:]]*\"0x[a-fA-F0-9]{40}\"/\"address\": \"${DVN_SOURCE_ADDRESS}\"/" \
            "$monitor_file" > "$temp_file"
    fi

    # Replace original file
    cat "$temp_file" > "$monitor_file"
    rm -f "$temp_file"

    log_info "oz-monitor config updated successfully"
}

# Output addresses in various formats for downstream use
output_addresses() {
    log_info "=== Contract Addresses Summary ==="
    echo ""
    echo "Source Chain ($SOURCE_CHAIN_ID):"
    echo "  DVN:     $DVN_SOURCE_ADDRESS"
    echo "  SendUln: $SEND_ULN_ADDRESS"
    echo ""
    echo "Destination Chain ($DEST_CHAIN_ID):"
    echo "  DVN:        $DVN_DEST_ADDRESS"
    echo "  ReceiveUln: $RECEIVE_ULN_ADDRESS"
    echo "  Settlement: $SETTLEMENT_ADDRESS"
    echo ""

    # Write addresses to a shell-sourceable file
    local env_file="${DEPLOY_DATA_DIR}/addresses.env"
    cat > "$env_file" << EOF
# Contract addresses - generated by post-deploy-config.sh
# Source: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Source Chain ($SOURCE_CHAIN_ID)
DVN_SOURCE_ADDRESS=$DVN_SOURCE_ADDRESS
SEND_ULN_ADDRESS=$SEND_ULN_ADDRESS
SOURCE_CHAIN_ID=$SOURCE_CHAIN_ID

# Destination Chain ($DEST_CHAIN_ID)
DVN_DEST_ADDRESS=$DVN_DEST_ADDRESS
RECEIVE_ULN_ADDRESS=$RECEIVE_ULN_ADDRESS
SETTLEMENT_ADDRESS=$SETTLEMENT_ADDRESS
DEST_CHAIN_ID=$DEST_CHAIN_ID
EOF

    log_info "Addresses written to: $env_file"
    log_info "Source with: source $env_file"
}

# Main execution
main() {
    log_info "=== Post-Deployment Configuration ==="
    log_info "Deploy data directory: $DEPLOY_DATA_DIR"
    log_info "Config file: $CONFIG_FILE"

    # Step 1: Wait for deploy state
    wait_for_deployment

    # Step 2: Read addresses from JSON files
    read_addresses

    # Step 3: Update operator config
    update_config

    # Step 4: Update oz-monitor config with DVN address
    update_oz_monitor_config

    # Step 5: Output addresses for verification
    output_addresses

    log_info "=== Post-Deployment Configuration Complete ==="
}

# Run main function
main "$@"
