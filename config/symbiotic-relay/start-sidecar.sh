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
    # Lowercase the address — relay_sidecar fails with checksummed (mixed-case) addresses
    DRIVER_ADDRESS=$(sed -n 's/.*"driver"[[:space:]]*:[[:space:]]*"\(0x[^"]*\)".*/\1/p' /deploy-data/relay_infra.json | tr '[:upper:]' '[:lower:]')
    DRIVER_CHAIN_ID=$(sed -n 's/.*"chainId"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' /deploy-data/relay_infra.json | head -1)
fi

if [ -z "${DRIVER_ADDRESS}" ]; then
    echo "ERROR: Could not extract driver address from relay_infra.json"
    exit 1
fi

echo "Driver address: ${DRIVER_ADDRESS}"
echo "Driver chain ID: ${DRIVER_CHAIN_ID}"

# Operator private key from per-operator env var (OPERATOR_N_PRIVATE_KEY)
eval "OPERATOR_KEY=\${OPERATOR_${OPERATOR_INDEX}_PRIVATE_KEY:-}"
if [ -z "${OPERATOR_KEY}" ]; then
    echo "ERROR: OPERATOR_${OPERATOR_INDEX}_PRIVATE_KEY is not set."
    echo "Run 'make setup' to generate operator keys."
    exit 1
fi
# Strip 0x prefix if present
PRIVATE_KEY_HEX="${OPERATOR_KEY#0x}"

# Secondary BLS key: primary key scalar + 10000 (deterministic from operator key).
# For large hex keys we add the offset to the last 8 hex chars (low 32 bits).
# This is safe because 10000 fits in 32 bits and overflow into higher digits is
# astronomically unlikely for random keys.
KEY_LEN=${#PRIVATE_KEY_HEX}
PREFIX_LEN=$((KEY_LEN - 8))
LAST8=$(echo "$PRIVATE_KEY_HEX" | cut -c$((PREFIX_LEN + 1))-)
PREFIX=$(echo "$PRIVATE_KEY_HEX" | cut -c1-${PREFIX_LEN})
LOW_DEC=$(printf "%d" "0x${LAST8}")
NEW_LOW_DEC=$((LOW_DEC + 10000))
NEW_LAST8=$(printf "%08x" $NEW_LOW_DEC)
SECONDARY_KEY_HEX="${PREFIX}${NEW_LAST8}"

# Swarm key for P2P (constant across network) - 64 hex chars = 32 bytes
# secp256k1 order n-1: FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140
SWARM_KEY="FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364140"

# Read chain IDs from deploy state (dynamic, works for both local and external)
SOURCE_CHAIN_ID=$(sed -n 's/.*"source_chain_id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' \
    /deploy-data/deploy-state.json | head -1)
# Destination chain must match the driver chain. deploy-state may contain
# historical providers and sed can otherwise pick a stale local chain id.
DEST_CHAIN_ID="${DRIVER_CHAIN_ID}"
SOURCE_CHAIN_ID="${SOURCE_CHAIN_ID:-31337}"
DEST_CHAIN_ID="${DEST_CHAIN_ID:-31338}"

echo "Source chain ID: ${SOURCE_CHAIN_ID}"
echo "Dest chain ID: ${DEST_CHAIN_ID}"

# Build secret keys string (keys without 0x prefix - sidecar expects raw hex)
# Format: type/network/tag/key
# - symb/0/15/: Primary BLS key for quorum signatures (tag 15 = BLS-BN254)
# - symb/0/11/: Secondary BLS key
# - symb/1/0/: Symbiotic network key
# - evm/1/<chain_id>/: ECDSA key for each chain
# - p2p/1/0/: Swarm key for libp2p (shared network topic)
# - p2p/1/1/: Identity key for libp2p (unique per node for peer discovery)
SECRET_KEYS="symb/0/15/${PRIVATE_KEY_HEX},symb/0/11/${SECONDARY_KEY_HEX},symb/1/0/${PRIVATE_KEY_HEX},evm/1/${SOURCE_CHAIN_ID}/${PRIVATE_KEY_HEX},evm/1/${DEST_CHAIN_ID}/${PRIVATE_KEY_HEX},p2p/1/0/${SWARM_KEY},p2p/1/1/${PRIVATE_KEY_HEX}"

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

# On external networks, driver exists only on settlement chain. Passing the
# source RPC can make relay_sidecar attempt driver calls on a chain with no code.
EVM_CHAINS="${EVM_SOURCE_RPC},${EVM_DEST_RPC}"
if [ "${SOURCE_CHAIN_ID}" != "31337" ] || [ "${DEST_CHAIN_ID}" != "31338" ]; then
    EVM_CHAINS="${EVM_DEST_RPC}"
fi

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
    --evm.chains "${EVM_CHAINS}"
