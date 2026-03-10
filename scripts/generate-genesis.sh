#!/bin/bash
set -euo pipefail

# Genesis bootstrap script for Symbiotic Relay
# This commits the initial validator set (epoch 0) to the Settlement contracts
# Without this, valset headers cannot be committed and DVN proofs cannot be verified

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
DEPLOY_DATA="$PROJECT_ROOT/data/deploy-data"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
FORCE_GENESIS="${FORCE_GENESIS:-0}"

# Load .env before common.sh so PRIVATE_KEY / RPC URLs are available
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/.env"
    set +a
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

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
        log_warn "FORCE_GENESIS=1 set, refreshing genesis regardless of on-chain epoch state"
        return 1
    fi

    if [ ! -f "$DEPLOY_DATA/relay_infra.json" ]; then
        return 1
    fi

    SETTLEMENT_ADDR=$(jq -r '.settlement // empty' "$DEPLOY_DATA/relay_infra.json")
    if [ -z "$SETTLEMENT_ADDR" ] || [ "$SETTLEMENT_ADDR" = "null" ]; then
        return 1
    fi

    RESULT=$(cast call "$SETTLEMENT_ADDR" \
        "getLastCommittedHeaderEpoch()(uint48)" \
        --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")

    if [ "$RESULT" != "0" ] && [ -n "$RESULT" ]; then
        log_info "Genesis already committed on-chain (latest committed epoch = $RESULT)"
        return 0
    fi

    return 1
}

# Fund relay keys on settlement chain
fund_relay_keys() {
    log_info "Funding relay keys on settlement chain..."

    # Deployer key
    DEPLOYER_KEY="${DEPLOYER_PRIVATE_KEY:-$PRIVATE_KEY}"

    # Relay sidecar keys use deterministic derivation
    # Base private key: configurable via OPERATOR_BASE_KEY (default: 1e18)
    BASE_KEY="${OPERATOR_BASE_KEY:-1000000000000000000}"
    OPERATOR_COUNT=3

    for i in $(seq 0 $((OPERATOR_COUNT - 1))); do
        # Calculate private key: BASE + i
        PRIV_KEY=$(printf "0x%064x" $((BASE_KEY + i)))
        OPERATOR_ADDR=$(cast wallet address --private-key "$PRIV_KEY")

        log_info "  Funding operator $i: $OPERATOR_ADDR"

        # Send ETH to each operator on settlement chain
        # Local: 1 ETH (free); Testnet: 0.2 ETH (keeps oz-relayer min-balance healthy)
        local fund_amount="1ether"
        if ! is_local; then fund_amount="${OPERATOR_FUND_AMOUNT:-0.2ether}"; fi
        cast send "$OPERATOR_ADDR" \
            --value "$fund_amount" \
            --rpc-url "$DEST_RPC" \
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

    DRIVER_ADDRESS=$(jq -r '.driver' "$DEPLOY_DATA/relay_infra.json" | tr '[:upper:]' '[:lower:]')
    if [ -z "$DRIVER_ADDRESS" ] || [ "$DRIVER_ADDRESS" = "null" ]; then
        log_error "Could not read Driver address from relay_infra.json"
        exit 1
    fi

    log_info "Driver address: $DRIVER_ADDRESS"

    # Genesis private key
    GENESIS_KEY="${GENESIS_PRIVATE_KEY:-$PRIVATE_KEY}"

    # Chain IDs from config
    local source_chain_id dest_chain_id
    source_chain_id="$(jq -r '.providers[.active_provider].source_chain_id // .providers[.active_provider].source_chain_selector // empty' "$ROOT_CONFIG_FILE" 2>/dev/null)"
    dest_chain_id="$(jq -r '.providers[.active_provider].destination_chain_id // .providers[.active_provider].destination_chain_selector // empty' "$ROOT_CONFIG_FILE" 2>/dev/null)"
    SOURCE_CHAIN_ID="${source_chain_id:-31337}"
    SETTLEMENT_CHAIN_ID="${dest_chain_id:-31338}"

    log_info "Generating genesis with:"
    log_info "  Source RPC: $SOURCE_RPC (chain $SOURCE_CHAIN_ID)"
    log_info "  Settlement RPC: $DEST_RPC (chain $SETTLEMENT_CHAIN_ID)"

    # Wait for Driver epoch system to start AND reach epoch >= 2.
    # relay_utils uses currentEpoch - 1 for genesis, so epoch 0 causes underflow.
    # We also need vault deposits to activate (SLASHING_WINDOW seconds = 1 vault epoch).
    if ! is_local; then
        log_info "Waiting for Driver epoch system to activate (need epoch >= 2)..."
        local epoch_wait=0
        local current_epoch=0
        while true; do
            current_epoch=$(cast call "$DRIVER_ADDRESS" "getCurrentEpoch()(uint48)" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")
            # Strip any whitespace/hex prefix artifacts
            current_epoch=$(echo "$current_epoch" | tr -d '[:space:]')
            if [ "$current_epoch" -ge 2 ] 2>/dev/null; then
                break
            fi
            sleep 10
            epoch_wait=$((epoch_wait + 10))
            if [ $epoch_wait -ge 900 ]; then
                log_error "Timeout waiting for Driver epoch >= 2 (15 min)"
                exit 1
            fi
            log_info "  Waiting for epoch >= 2... current=$current_epoch (${epoch_wait}s elapsed)"
        done
        log_info "Driver epoch system active (current epoch: $current_epoch)"
    fi

    # Retry loop - voting power snapshots need time to propagate
    MAX_RETRIES=60
    RETRY_DELAY=5

    # Use the same relay image as docker-compose.yml
    RELAY_IMAGE="${RELAY_IMAGE:-symbioticfi/relay:1.0.1-20260305162153-f333c1a4e45c}"

    if is_local; then
        # Local: use Docker network and container names for RPCs
        NETWORK_NAME=$(docker network ls --filter "name=bridge-network" --format "{{.Name}}" | grep -E "_bridge-network$" | head -1)
        if [ -z "$NETWORK_NAME" ]; then
            log_error "Could not find bridge-network. Make sure Docker Compose services are running."
            exit 1
        fi
        log_info "Using Docker network: $NETWORK_NAME"

        for attempt in $(seq 1 $MAX_RETRIES); do
            log_info "Genesis attempt $attempt/$MAX_RETRIES..."

            if docker run --rm \
                --network "$NETWORK_NAME" \
                $RELAY_IMAGE \
                /app/relay_utils network \
                    --chains "http://anvil:8545,http://anvil-settlement:8546" \
                    --driver.address "$DRIVER_ADDRESS" \
                    --driver.chainid "$SETTLEMENT_CHAIN_ID" \
                generate-genesis \
                    --commit \
                    --secret-keys "$SOURCE_CHAIN_ID:$GENESIS_KEY,$SETTLEMENT_CHAIN_ID:$GENESIS_KEY" 2>&1; then
                log_info "Genesis committed successfully"
                return 0
            fi

            if [ $attempt -lt $MAX_RETRIES ]; then
                log_warn "Genesis failed, retrying in ${RETRY_DELAY}s... (voting power may not be captured yet)"
                sleep $RETRY_DELAY
            fi
        done
    else
        # External: only pass the settlement chain RPC.
        # relay_utils will try to call the Driver on every chain in --chains,
        # so including the source chain (where the Driver doesn't exist) causes
        # "no contract code at given address" errors.
        for attempt in $(seq 1 $MAX_RETRIES); do
            log_info "Genesis attempt $attempt/$MAX_RETRIES..."

            if docker run --rm \
                $RELAY_IMAGE \
                /app/relay_utils network \
                    --chains "$DEST_RPC" \
                    --driver.address "$DRIVER_ADDRESS" \
                    --driver.chainid "$SETTLEMENT_CHAIN_ID" \
                generate-genesis \
                    --commit \
                    --secret-keys "$SETTLEMENT_CHAIN_ID:$GENESIS_KEY" 2>&1; then
                log_info "Genesis committed successfully"
                return 0
            fi

            if [ $attempt -lt $MAX_RETRIES ]; then
                log_warn "Genesis failed, retrying in ${RETRY_DELAY}s... (voting power may not be captured yet)"
                sleep $RETRY_DELAY
            fi
        done
    fi

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
        "getLastCommittedHeaderEpoch()(uint48)" \
        --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")

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
