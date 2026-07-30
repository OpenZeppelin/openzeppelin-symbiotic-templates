#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts"
DEPLOY_DATA_ROOT="$CONTRACTS_DIR/deploy-data"
DEPLOY_DATA_DIR="$DEPLOY_DATA_ROOT/chainlink"
PORT_ONE="${CCV_VALIDATE_PORT_ONE:-18545}"
PORT_TWO="${CCV_VALIDATE_PORT_TWO:-18546}"
RPC_ONE="http://127.0.0.1:$PORT_ONE"
RPC_TWO="http://127.0.0.1:$PORT_TWO"

DEPLOYER_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
# Anvil account 9 is reserved for factory deployment and unused by all other local roles.
FACTORY_PRIVATE_KEY="0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6"
FACTORY_DEPLOYER="0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"

for command in anvil cast forge jq; do
    command -v "$command" >/dev/null || {
        echo "required command not found: $command" >&2
        exit 1
    }
done

if cast block-number --rpc-url "$RPC_ONE" >/dev/null 2>&1; then
    echo "port $PORT_ONE is already in use" >&2
    exit 1
fi
if cast block-number --rpc-url "$RPC_TWO" >/dev/null 2>&1; then
    echo "port $PORT_TWO is already in use" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d)"

# contracts/deploy-data is env-scoped via an xtask-managed symlink (see
# xtask::context::ensure_deploy_data_env_link). Record whether it existed
# before this script ran (absent / symlink / plain dir) so cleanup can
# restore that exact state rather than leaving behind a plain directory
# `mkdir -p` created for a fresh checkout.
DEPLOY_DATA_ROOT_PREEXISTED=0
if [[ -e "$DEPLOY_DATA_ROOT" || -L "$DEPLOY_DATA_ROOT" ]]; then
    DEPLOY_DATA_ROOT_PREEXISTED=1
fi

mkdir -p "$DEPLOY_DATA_DIR"

backup_artifact() {
    local name="$1"
    if [[ -f "$DEPLOY_DATA_DIR/$name" ]]; then
        cp "$DEPLOY_DATA_DIR/$name" "$TMP_DIR/original-$name"
    fi
}

restore_artifact() {
    local name="$1"
    if [[ -f "$TMP_DIR/original-$name" ]]; then
        cp "$TMP_DIR/original-$name" "$DEPLOY_DATA_DIR/$name"
    else
        rm -f "$DEPLOY_DATA_DIR/$name"
    fi
}

backup_artifact ccv_factory.json
backup_artifact ccv_resolver.json

PID_ONE=""
PID_TWO=""
cleanup() {
    [[ -z "$PID_ONE" ]] || kill "$PID_ONE" >/dev/null 2>&1 || true
    [[ -z "$PID_TWO" ]] || kill "$PID_TWO" >/dev/null 2>&1 || true
    [[ -z "$PID_ONE" ]] || wait "$PID_ONE" 2>/dev/null || true
    [[ -z "$PID_TWO" ]] || wait "$PID_TWO" 2>/dev/null || true
    restore_artifact ccv_factory.json
    restore_artifact ccv_resolver.json
    if [[ "$DEPLOY_DATA_ROOT_PREEXISTED" -eq 0 ]]; then
        rm -rf "$DEPLOY_DATA_ROOT"
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

anvil --silent --port "$PORT_ONE" --chain-id 31337 >"$TMP_DIR/anvil-one.log" 2>&1 &
PID_ONE=$!
anvil --silent --port "$PORT_TWO" --chain-id 31338 >"$TMP_DIR/anvil-two.log" 2>&1 &
PID_TWO=$!

wait_for_rpc() {
    local rpc_url="$1"
    for _ in $(seq 1 50); do
        if cast block-number --rpc-url "$rpc_url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    echo "anvil did not become ready at $rpc_url" >&2
    return 1
}

deploy_resolver() {
    local label="$1"
    local rpc_url="$2"

    [[ "$(cast nonce "$FACTORY_DEPLOYER" --rpc-url "$rpc_url")" == "0" ]] || {
        echo "$label factory deployer nonce is not zero" >&2
        return 1
    }

    (
        cd "$CONTRACTS_DIR"
        CCV_FACTORY_DEPLOYER="$FACTORY_DEPLOYER" forge script script/chainlink/DeployCCV.s.sol:DeployCCV \
            --sig "deployFactory(address[])" "[$DEPLOYER_ADDRESS]" \
            --rpc-url "$rpc_url" --broadcast --private-key "$FACTORY_PRIVATE_KEY" \
            --non-interactive --quiet
    )
    cp "$DEPLOY_DATA_DIR/ccv_factory.json" "$TMP_DIR/$label-factory.json"

    (
        cd "$CONTRACTS_DIR"
        DEPLOYER_ADDRESS="$DEPLOYER_ADDRESS" CCV_RESOLVER_OWNER="$DEPLOYER_ADDRESS" \
            forge script script/chainlink/DeployCCV.s.sol:DeployCCV \
            --sig "deployResolver(address)" "$DEPLOYER_ADDRESS" \
            --rpc-url "$rpc_url" --broadcast --private-key "$DEPLOYER_PRIVATE_KEY" \
            --non-interactive --quiet
    )
    cp "$DEPLOY_DATA_DIR/ccv_resolver.json" "$TMP_DIR/$label-resolver.json"
}

wait_for_rpc "$RPC_ONE"
wait_for_rpc "$RPC_TWO"
deploy_resolver one "$RPC_ONE"
deploy_resolver two "$RPC_TWO"

FACTORY_ONE="$(jq -r '.factory | ascii_downcase' "$TMP_DIR/one-factory.json")"
FACTORY_TWO="$(jq -r '.factory | ascii_downcase' "$TMP_DIR/two-factory.json")"
RESOLVER_ONE="$(jq -r '.resolver | ascii_downcase' "$TMP_DIR/one-resolver.json")"
RESOLVER_TWO="$(jq -r '.resolver | ascii_downcase' "$TMP_DIR/two-resolver.json")"

[[ "$FACTORY_ONE" == "$FACTORY_TWO" ]] || {
    echo "factory address mismatch: $FACTORY_ONE != $FACTORY_TWO" >&2
    exit 1
}
[[ "$RESOLVER_ONE" == "$RESOLVER_TWO" ]] || {
    echo "resolver address mismatch: $RESOLVER_ONE != $RESOLVER_TWO" >&2
    exit 1
}

EXPECTED_OWNER="$(printf '%s' "$DEPLOYER_ADDRESS" | tr '[:upper:]' '[:lower:]')"
OWNER_ONE="$(cast call "$RESOLVER_ONE" "owner()(address)" --rpc-url "$RPC_ONE" | tr '[:upper:]' '[:lower:]')"
OWNER_TWO="$(cast call "$RESOLVER_TWO" "owner()(address)" --rpc-url "$RPC_TWO" | tr '[:upper:]' '[:lower:]')"
[[ "$OWNER_ONE" == "$EXPECTED_OWNER" ]] || {
    echo "resolver owner mismatch on first chain: $OWNER_ONE != $EXPECTED_OWNER" >&2
    exit 1
}
[[ "$OWNER_TWO" == "$EXPECTED_OWNER" ]] || {
    echo "resolver owner mismatch on second chain: $OWNER_TWO != $EXPECTED_OWNER" >&2
    exit 1
}

echo "resolver determinism validated"
echo "factory: $FACTORY_ONE"
echo "resolver: $RESOLVER_ONE"
echo "owner: $OWNER_ONE"
