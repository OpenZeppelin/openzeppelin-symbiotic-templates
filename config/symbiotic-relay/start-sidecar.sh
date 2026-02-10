#!/bin/sh
# Start Symbiotic relay sidecar with operator-specific configuration
#
# Usage: start-sidecar.sh <operator_index>
# Example: start-sidecar.sh 1

set -e

OPERATOR_INDEX="${1:-1}"
STORAGE_DIR="/storage"
CONFIG_DIR="/config"
MARKER_TIMEOUT="${MARKER_TIMEOUT:-300}"

echo "=== Starting Relay Sidecar ${OPERATOR_INDEX} ==="

# Wait for deploy-state.json with timeout.
echo "Waiting for deploy state (timeout: ${MARKER_TIMEOUT}s)..."
elapsed=0
while [ ! -f /deploy-data/deploy-state.json ]; do
    if [ $elapsed -ge $MARKER_TIMEOUT ]; then
        echo "ERROR: Timeout waiting for deploy-state.json after ${MARKER_TIMEOUT}s"
        exit 1
    fi
    sleep 2
    elapsed=$((elapsed + 2))
    if [ $((elapsed % 10)) -eq 0 ]; then
        echo "Still waiting for deploy state... (${elapsed}s elapsed)"
    fi
done
echo "Deploy state found!"

# Wait for relay infrastructure data (Driver, KeyRegistry, etc.) with timeout.
echo "Waiting for relay infrastructure data (timeout: ${MARKER_TIMEOUT}s)..."
elapsed=0
while [ ! -f /deploy-data/relay_infra.json ]; do
    if [ $elapsed -ge $MARKER_TIMEOUT ]; then
        echo "ERROR: Timeout waiting for relay_infra.json after ${MARKER_TIMEOUT}s"
        exit 1
    fi
    sleep 2
    elapsed=$((elapsed + 2))
    if [ $((elapsed % 10)) -eq 0 ]; then
        echo "Still waiting for relay infrastructure data... (${elapsed}s elapsed)"
    fi
done
echo "Relay infrastructure data found!"

echo "All contracts deployed, extracting driver address..."

# Extract driver address from relay_infra.json
DRIVER_ADDRESS=""
if [ -f /deploy-data/relay_infra.json ]; then
    # Use sed to extract driver address (jq not available in alpine by default)
    DRIVER_ADDRESS=$(sed -n 's/.*"driver"[[:space:]]*:[[:space:]]*"\(0x[^"]*\)".*/\1/p' /deploy-data/relay_infra.json)
    DRIVER_CHAIN_ID=$(sed -n 's/.*"chainId"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' /deploy-data/relay_infra.json | head -1)
fi

if [ -z "${DRIVER_ADDRESS}" ]; then
    echo "ERROR: Could not extract driver address from relay_infra.json"
    exit 1
fi

echo "Driver address: ${DRIVER_ADDRESS}"
echo "Driver chain ID: ${DRIVER_CHAIN_ID}"

# Deterministic key generation (same as symbiotic-super-sum)
# Base private key: 1000000000000000000 (1e18)
BASE_KEY=1000000000000000000
KEY_INDEX=$((OPERATOR_INDEX - 1))
PRIVATE_KEY_DECIMAL=$((BASE_KEY + KEY_INDEX))
SECONDARY_KEY_DECIMAL=$((BASE_KEY + KEY_INDEX + 10000))

# Convert to hex (64 chars, zero-padded)
PRIVATE_KEY_HEX=$(printf "%064x" $PRIVATE_KEY_DECIMAL)
SECONDARY_KEY_HEX=$(printf "%064x" $SECONDARY_KEY_DECIMAL)

# Swarm key for P2P (constant across network) - 64 hex chars = 32 bytes
# secp256k1 order n-1: FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140
SWARM_KEY="FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140"

# Build secret keys string (keys without 0x prefix - sidecar expects raw hex)
# Format: type/network/tag/key
# - symb/0/15/: Primary BLS key for quorum signatures (tag 15 = BLS-BN254)
# - symb/0/11/: Secondary BLS key
# - symb/1/0/: Symbiotic network key
# - evm/1/31337/: ECDSA key for source chain
# - evm/1/31338/: ECDSA key for dest chain
# - p2p/1/0/: Swarm key for libp2p (shared network topic)
# - p2p/1/1/: Identity key for libp2p (unique per node for peer discovery)
SECRET_KEYS="symb/0/15/${PRIVATE_KEY_HEX},symb/0/11/${SECONDARY_KEY_HEX},symb/1/0/${PRIVATE_KEY_HEX},evm/1/31337/${PRIVATE_KEY_HEX},evm/1/31338/${PRIVATE_KEY_HEX},p2p/1/0/${SWARM_KEY},p2p/1/1/${PRIVATE_KEY_HEX}"

# Override with env var if provided
if [ -n "${SIDECAR_SECRET_KEYS}" ]; then
    SECRET_KEYS="${SIDECAR_SECRET_KEYS}"
fi

echo "Operator Index: ${OPERATOR_INDEX}"
echo "Private Key (first 8 hex): ${PRIVATE_KEY_HEX:0:8}..."

# EVM chain RPC URLs (from environment or defaults)
# Note: Aggregator role is determined by P2P consensus, not forced
EVM_SOURCE_RPC="${EVM_SOURCE_RPC:-http://anvil:8545}"
EVM_DEST_RPC="${EVM_DEST_RPC:-http://anvil-settlement:8546}"

echo "Starting relay sidecar with driver at ${DRIVER_ADDRESS} on chain ${DRIVER_CHAIN_ID}"

# Start the relay binary with driver configuration
# Note: Using mDNS for peer discovery on local network (bootnodes removed - peer ID changes per run)
exec /app/relay_sidecar \
    --secret-keys "${SECRET_KEYS}" \
    --storage-dir "${STORAGE_DIR}" \
    --api.listen "0.0.0.0:8080" \
    --p2p.listen "/ip4/0.0.0.0/tcp/8880" \
    --p2p.mdns \
    --p2p.dht-mode disabled \
    --sync.period 30s \
    --driver.chain-id "${DRIVER_CHAIN_ID}" \
    --driver.address "${DRIVER_ADDRESS}" \
    --evm.chains "${EVM_SOURCE_RPC},${EVM_DEST_RPC}"
