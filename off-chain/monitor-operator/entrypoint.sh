#!/bin/bash
# Entrypoint script for Symbiotic DVN Monitor
# Generates OZ Monitor config from deploy-data and starts the monitor

set -euo pipefail

echo "=== Symbiotic LayerZero DVN Monitor ==="

# Wait for deploy-data to be available
echo "Waiting for deploy-data..."
while [ ! -f /deploy-data/source_chain_contracts.json ] || [ ! -f /deploy-data/dest_chain_contracts.json ]; do
    sleep 2
done
echo "Deploy data found!"

# Read DVN addresses from deploy-data
SOURCE_DVN_ADDRESS=$(jq -r '.dvn.addr' /deploy-data/source_chain_contracts.json)
DEST_DVN_ADDRESS=$(jq -r '.dvn.addr' /deploy-data/dest_chain_contracts.json)

echo "Source DVN: $SOURCE_DVN_ADDRESS"
echo "Dest DVN: $DEST_DVN_ADDRESS"

# Export for DVN worker script
export SOURCE_DVN_ADDRESS
export DEST_DVN_ADDRESS

# Generate monitor config from template
echo "Generating monitor config..."
sed "s/{{SOURCE_DVN_ADDRESS}}/$SOURCE_DVN_ADDRESS/g" \
    /app/config/monitors/dvn_source_monitor.json.template \
    > /app/config/monitors/dvn_source_monitor.json

echo "Configuration generated!"
echo "Starting OpenZeppelin Monitor..."

# Start OZ Monitor (the default entrypoint from base image)
exec /app/openzeppelin-monitor
