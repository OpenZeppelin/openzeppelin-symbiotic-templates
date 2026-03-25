#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Test that ENV propagates to deploy target
deploy_plan="$(cd "$REPO_ROOT" && make -n deploy ENV=testnet)"

echo "$deploy_plan" | grep -F -- "--env testnet" >/dev/null || {
    echo "expected deploy target to pass --env testnet to xtask" >&2
    exit 1
}

echo "$deploy_plan" | grep -F -- "--env-config config/environments/testnet.json" >/dev/null || {
    echo "expected deploy target to pass --env-config for testnet" >&2
    exit 1
}

echo "$deploy_plan" | grep -F -- "--deployments deployments/testnet.json" >/dev/null || {
    echo "expected deploy target to pass --deployments for testnet" >&2
    exit 1
}

echo "$deploy_plan" | grep -F -- "--generated-dir generated/testnet" >/dev/null || {
    echo "expected deploy target to pass --generated-dir for testnet" >&2
    exit 1
}

echo "$deploy_plan" | grep -F "cargo xtask" >/dev/null || {
    echo "expected deploy target to call cargo xtask" >&2
    exit 1
}

echo "make deploy propagation test passed"
