#!/bin/bash
set -euo pipefail

# Genesis bootstrap script for Symbiotic Relay
# This commits the initial validator set (epoch 0) to the Settlement contracts
# Without this, valset headers cannot be committed and DVN proofs cannot be verified

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
DEPLOY_DATA="$PROJECT_ROOT/data/deploy-data"
FORCE_GENESIS="${FORCE_GENESIS:-0}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Wait for relay infrastructure deployment
wait_for_deployment() {
    log_info "Waiting for relay infrastructure deployment..."
    local timeout=60
    local elapsed=0

    while [ ! -f "$DEPLOY_DATA/relay_infra.json" ]; do
        sleep 2
        elapsed=$((elapsed + 2))
        if [ $elapsed -ge $timeout ]; then
            log_error "Timeout waiting for relay_infra.json"
            exit 1
        fi
    done

    log_info "Relay infrastructure deployment found"
}

# Check if genesis already committed
check_genesis_exists() {
    if [ "$FORCE_GENESIS" = "1" ]; then
        log_warn "FORCE_GENESIS=1 set, refreshing genesis regardless of marker"
        return 1
    fi

    if [ -f "$DEPLOY_DATA/genesis-complete.marker" ]; then
        log_info "Genesis already committed (marker file exists)"
        return 0
    fi
    return 1
}

# Fund relay keys on settlement chain
fund_relay_keys() {
    log_info "Funding relay keys on settlement chain..."

    # Deployer key (has ETH on both chains from Anvil genesis)
    DEPLOYER_KEY="${DEPLOYER_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"

    # Relay sidecar keys use deterministic derivation
    # Base private key: 1e18 (1000000000000000000)
    # Each operator: BASE + operator_index
    BASE_KEY=1000000000000000000
    OPERATOR_COUNT=3

    for i in $(seq 0 $((OPERATOR_COUNT - 1))); do
        # Calculate private key: BASE + i
        PRIV_KEY=$(printf "0x%064x" $((BASE_KEY + i)))
        OPERATOR_ADDR=$(cast wallet address --private-key "$PRIV_KEY")

        log_info "  Funding operator $i: $OPERATOR_ADDR"

        # Send 1 ETH to each operator on settlement chain
        cast send "$OPERATOR_ADDR" \
            --value 1ether \
            --rpc-url http://localhost:8546 \
            --private-key "$DEPLOYER_KEY" \
            >/dev/null 2>&1 || {
                log_warn "Failed to fund operator $i (may already be funded)"
            }
    done

    log_info "Relay keys funded on settlement chain"
}

# Main genesis generation with retry logic
generate_genesis() {
    # Read Driver address from deployment
    if [ ! -f "$DEPLOY_DATA/relay_infra.json" ]; then
        log_error "relay_infra.json not found. Deploy relay infrastructure first."
        exit 1
    fi

    DRIVER_ADDRESS=$(jq -r '.driver' "$DEPLOY_DATA/relay_infra.json")
    if [ -z "$DRIVER_ADDRESS" ] || [ "$DRIVER_ADDRESS" = "null" ]; then
        log_error "Could not read Driver address from relay_infra.json"
        exit 1
    fi

    log_info "Driver address: $DRIVER_ADDRESS"

    # Genesis private key (Anvil account 0 - deployer)
    GENESIS_KEY="${GENESIS_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"

    # RPC URLs (use Docker network names if running in Docker, localhost otherwise)
    SOURCE_RPC="${SOURCE_RPC_URL:-http://localhost:8545}"
    SETTLEMENT_RPC="${SETTLEMENT_RPC_URL:-http://localhost:8546}"

    # Chain IDs
    SOURCE_CHAIN_ID="${SOURCE_CHAIN_ID:-31337}"
    SETTLEMENT_CHAIN_ID="${SETTLEMENT_CHAIN_ID:-31338}"

    log_info "Generating genesis with:"
    log_info "  Source RPC: $SOURCE_RPC (chain $SOURCE_CHAIN_ID)"
    log_info "  Settlement RPC: $SETTLEMENT_RPC (chain $SETTLEMENT_CHAIN_ID)"

    # Note: Docker Compose prefixes network names with project name (e.g., projectname_bridge-network)
    NETWORK_NAME=$(docker network ls --filter "name=bridge-network" --format "{{.Name}}" | grep -E "_bridge-network$" | head -1)
    if [ -z "$NETWORK_NAME" ]; then
        log_error "Could not find bridge-network. Make sure Docker Compose services are running."
        exit 1
    fi
    log_info "Using Docker network: $NETWORK_NAME"

    # Retry loop - voting power snapshots need time to propagate
    # Similar to symbiotic-super-sum's genesis-generator.sh approach
    MAX_RETRIES=30
    RETRY_DELAY=2

    for attempt in $(seq 1 $MAX_RETRIES); do
        log_info "Genesis attempt $attempt/$MAX_RETRIES..."

        if docker run --rm \
            --network "$NETWORK_NAME" \
            symbioticfi/relay:latest \
            /app/relay_utils network \
                --chains "http://anvil:8545,http://anvil-settlement:8546" \
                --driver.address "$DRIVER_ADDRESS" \
                --driver.chainid "$SETTLEMENT_CHAIN_ID" \
            generate-genesis \
                --commit \
                --secret-keys "$SOURCE_CHAIN_ID:$GENESIS_KEY,$SETTLEMENT_CHAIN_ID:$GENESIS_KEY" 2>&1; then
            log_info "Genesis committed successfully"
            date > "$DEPLOY_DATA/genesis-complete.marker"
            return 0
        fi

        if [ $attempt -lt $MAX_RETRIES ]; then
            log_warn "Genesis failed, retrying in ${RETRY_DELAY}s... (voting power may not be captured yet)"
            sleep $RETRY_DELAY
        fi
    done

    log_error "Genesis failed after $MAX_RETRIES attempts"
    exit 1
}

# Verify genesis was committed
verify_genesis() {
    log_info "Verifying genesis commitment..."

    # Read Settlement address from relay infrastructure
    if [ ! -f "$DEPLOY_DATA/relay_infra.json" ]; then
        log_warn "relay_infra.json not found, skipping verification"
        return 0
    fi

    SETTLEMENT_ADDR=$(jq -r '.settlement' "$DEPLOY_DATA/relay_infra.json")

    # Check if valset header is committed (capture timestamp should be non-zero)
    # This uses the Settlement contract's getter for the latest committed epoch
    RESULT=$(cast call "$SETTLEMENT_ADDR" \
        "getLatestCommittedEpoch()(uint64)" \
        --rpc-url "http://localhost:8546" 2>/dev/null || echo "0")

    if [ "$RESULT" != "0" ] && [ -n "$RESULT" ]; then
        log_info "Genesis verified: committed epoch = $RESULT"
    else
        log_warn "Could not verify genesis commitment (this may be expected if Settlement doesn't have this getter)"
    fi
}

# Main
main() {
    log_info "=== Symbiotic Relay Genesis Bootstrap ==="

    # Check if already done
    if check_genesis_exists; then
        verify_genesis
        exit 0
    fi

    # Wait for deployment
    wait_for_deployment

    # Fund relay keys on settlement chain (needed for valset commits)
    fund_relay_keys

    # Generate and commit genesis
    generate_genesis

    # Verify
    verify_genesis

    log_info "=== Genesis Bootstrap Complete ==="
}

main "$@"
