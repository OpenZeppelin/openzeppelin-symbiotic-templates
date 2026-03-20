#!/usr/bin/env bash
# publish-addresses.sh — Extract deployed addresses from Forge output into deployments JSON.
#
# Reads individual JSON files from contracts/deploy-data/ (written by Forge scripts
# via vm.writeJson) and populates deployments/<env>.json.
#
# Usage: ./scripts/publish-addresses.sh [ENV_CONFIG]
# Example: ./scripts/publish-addresses.sh config/environments/local.json
#
# The script is idempotent — running it again overwrites existing deployment entries.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=lib/env-config.sh
source "$SCRIPT_DIR/lib/env-config.sh"

# Allow override via argument
if [[ -n "${1:-}" ]]; then
    export ENV_CONFIG="$1"
fi

DEPLOY_DATA="$PROJECT_ROOT/contracts/deploy-data"
DEPLOYMENTS_FILE="$(deployments_file)"

ensure_deployments_file

echo "Publishing addresses to: $DEPLOYMENTS_FILE"
echo "Reading from: $DEPLOY_DATA/"

# Track what we publish
published=0

# --- DVN deployments ---

if [[ -f "$DEPLOY_DATA/source_contracts.json" ]]; then
    echo "  Publishing source DVN deployment..."
    dvn=$(jq -r '.dvn' "$DEPLOY_DATA/source_contracts.json")
    env_set_deployment source dvn "$dvn"
    published=$((published + 1))
fi

if [[ -f "$DEPLOY_DATA/dest_contracts.json" ]]; then
    echo "  Publishing destination DVN deployment..."
    dvn=$(jq -r '.dvn' "$DEPLOY_DATA/dest_contracts.json")
    env_set_deployment destination dvn "$dvn"
    published=$((published + 1))
fi

# --- Relay infrastructure ---

if [[ -f "$DEPLOY_DATA/relay_infra.json" ]]; then
    echo "  Publishing relay infrastructure deployment..."
    relay_infra=$(jq '{
        settlement: .settlement,
        driver: .driver,
        keyRegistry: .keyRegistry,
        votingPowers: .votingPowers,
        network: .network,
        stakingToken: .stakingToken
    }' "$DEPLOY_DATA/relay_infra.json")
    env_set_deployment_object destination relayInfra "$relay_infra"
    published=$((published + 1))
fi

# --- Test OApp ---

if [[ -f "$DEPLOY_DATA/testoapp_source.json" ]]; then
    echo "  Publishing source TestOApp deployment..."
    oapp=$(jq -r '.testOApp' "$DEPLOY_DATA/testoapp_source.json")
    env_set_deployment source testOApp "$oapp"
    published=$((published + 1))
fi

if [[ -f "$DEPLOY_DATA/testoapp_dest.json" ]]; then
    echo "  Publishing destination TestOApp deployment..."
    oapp=$(jq -r '.testOApp' "$DEPLOY_DATA/testoapp_dest.json")
    env_set_deployment destination testOApp "$oapp"
    published=$((published + 1))
fi

# --- Chainlink CCV deployments (if present) ---

if [[ -f "$DEPLOY_DATA/ccv_source_contracts.json" ]]; then
    echo "  Publishing source CCV deployment..."
    ccv_source=$(jq '{
        ccv: .ccv,
        onRamp: .onRamp,
        offRamp: .offRamp
    } | with_entries(select(.value != null))' "$DEPLOY_DATA/ccv_source_contracts.json")
    env_set_deployment_object source chainlinkCcv "$ccv_source"
    published=$((published + 1))
fi

if [[ -f "$DEPLOY_DATA/ccv_dest_contracts.json" ]]; then
    echo "  Publishing destination CCV deployment..."
    ccv_dest=$(jq '{
        ccv: .ccv,
        onRamp: .onRamp,
        offRamp: .offRamp,
        settlement: .settlement
    } | with_entries(select(.value != null))' "$DEPLOY_DATA/ccv_dest_contracts.json")
    env_set_deployment_object destination chainlinkCcv "$ccv_dest"
    published=$((published + 1))
fi

echo "Published $published address group(s) to $DEPLOYMENTS_FILE"

# Generate sidecar env for Docker Compose/runtime consumers
env_generate_compose_env
