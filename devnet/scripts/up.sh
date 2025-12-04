#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Start Symbiotic LayerZero DVN Devnet
# =============================================================================

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
DEVNET_DIR=$(dirname "$SCRIPT_DIR")
PROJECT_ROOT=$(dirname "$DEVNET_DIR")

cd "$PROJECT_ROOT"

echo "=== Starting Symbiotic LayerZero DVN Devnet ==="
echo ""

# Check if relay-config exists
if [ ! -d "devnet/relay-config" ]; then
    echo "Relay config not found. Running generate_network.sh..."
    ./generate_network.sh
fi

# Start docker compose
cd devnet
echo "Starting services..."
docker compose up -d --build

echo ""
echo "Services started. Waiting for chains to be ready..."
sleep 5

# Check health
echo ""
echo "Checking service health..."
docker compose ps

echo ""
echo "=== Devnet Started ==="
echo ""
echo "Watch deployment logs:"
echo "  docker compose logs -f deployer"
echo ""
echo "Watch DVN worker logs:"
echo "  docker compose logs -f dvn-worker"
echo ""
echo "RPC Endpoints:"
echo "  Source Chain (31337): http://localhost:8545"
echo "  Dest Chain (31338):   http://localhost:8546"
echo ""
echo "Sidecar Endpoints:"
echo "  Sidecar 1: http://localhost:8081"
echo "  Sidecar 2: http://localhost:8082"
echo "  Sidecar 3: http://localhost:8083"
echo "  Sidecar 4: http://localhost:8084"
echo ""
