#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Test that ENV propagates to configure target
configure_plan="$(cd "$REPO_ROOT" && make -n configure ENV=testnet)"

echo "$configure_plan" | grep -F "ENV=testnet" >/dev/null || {
    echo "expected configure target to pass ENV=testnet" >&2
    exit 1
}

echo "$configure_plan" | grep -F "ENV_CONFIG=config/environments/testnet.json" >/dev/null || {
    echo "expected configure target to pass ENV_CONFIG for testnet" >&2
    exit 1
}

echo "$configure_plan" | grep -F "DEPLOYMENTS_FILE=deployments/testnet.json" >/dev/null || {
    echo "expected configure target to pass DEPLOYMENTS_FILE for testnet" >&2
    exit 1
}

echo "$configure_plan" | grep -F "GENERATED_DIR=generated/testnet" >/dev/null || {
    echo "expected configure target to pass GENERATED_DIR for testnet" >&2
    exit 1
}

echo "$configure_plan" | grep -F "generate_oz_configs" >/dev/null || {
    echo "expected configure target to call generate_oz_configs" >&2
    exit 1
}

echo "make config propagation test passed"
