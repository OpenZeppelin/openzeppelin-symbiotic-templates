#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CUSTOM_ROOT_CONFIG="/tmp/custom-root-config.json"

configure_plan="$(cd "$REPO_ROOT" && make -n configure ROOT_CONFIG_FILE="$CUSTOM_ROOT_CONFIG")"
addresses_plan="$(cd "$REPO_ROOT" && make -n addresses ROOT_CONFIG_FILE="$CUSTOM_ROOT_CONFIG")"

echo "$configure_plan" | grep -F "ROOT_CONFIG_FILE=$CUSTOM_ROOT_CONFIG ./scripts/generate-configs.sh" >/dev/null || {
    echo "expected configure target to pass ROOT_CONFIG_FILE to generate-configs.sh" >&2
    exit 1
}

echo "$configure_plan" | grep -F "ROOT_CONFIG_FILE=$CUSTOM_ROOT_CONFIG ./scripts/generate-addresses.sh" >/dev/null || {
    echo "expected configure target to pass ROOT_CONFIG_FILE to generate-addresses.sh" >&2
    exit 1
}

echo "$addresses_plan" | grep -F "ROOT_CONFIG_FILE=$CUSTOM_ROOT_CONFIG ./scripts/generate-addresses.sh" >/dev/null || {
    echo "expected addresses target to pass ROOT_CONFIG_FILE to generate-addresses.sh" >&2
    exit 1
}

echo "make root-config propagation test passed"
