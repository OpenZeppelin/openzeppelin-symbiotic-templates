#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
ROOT_CONFIG_FILE="${ROOT_CONFIG_FILE:-$PROJECT_ROOT/config/root.config.json}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
DEPLOY_DATA="${DEPLOY_DATA:-$PROJECT_ROOT/data/deploy-data}"

if [[ "$ROOT_CONFIG_FILE" != /* ]]; then
    ROOT_CONFIG_FILE="$PROJECT_ROOT/$ROOT_CONFIG_FILE"
fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

die() {
    echo "ERROR: $*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"
}

require_file() {
    [[ -f "$1" ]] || die "missing file: $1"
}

main() {
    require_cmd jq
    require_cmd cast
    require_cmd forge

    require_file "$ROOT_CONFIG_FILE"
    require_file "$DEPLOY_DATA/ccv_source_contracts.json"
    require_file "$DEPLOY_DATA/ccv_dest_contracts.json"

    local ccv_mode
    ccv_mode="$(get_ccv_mode)"
    if [[ "$ccv_mode" != "symbiotic_mock" ]]; then
        die "unsupported providers.chainlink_ccv.mode '$ccv_mode' (expected symbiotic_mock)"
    fi

    if ! cast client --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        die "source chain is not reachable at http://localhost:8545"
    fi
    if ! cast client --rpc-url http://localhost:8546 >/dev/null 2>&1; then
        die "destination chain is not reachable at http://localhost:8546"
    fi

    local src_ccv dst_ccv src_selector dst_selector src_onramp src_offramp dst_onramp dst_offramp
    src_ccv="$(jq -r '.ccv' "$DEPLOY_DATA/ccv_source_contracts.json")"
    dst_ccv="$(jq -r '.ccv' "$DEPLOY_DATA/ccv_dest_contracts.json")"
    src_selector="$(get_ccv_source_chain_selector)"
    dst_selector="$(get_ccv_dest_chain_selector)"
    src_onramp="$(get_ccv_source_onramp_address 2>/dev/null || true)"
    src_offramp="$(get_ccv_source_offramp_address 2>/dev/null || true)"
    dst_onramp="$(get_ccv_dest_onramp_address 2>/dev/null || true)"
    dst_offramp="$(get_ccv_dest_offramp_address 2>/dev/null || true)"

    [[ -n "$src_onramp" ]] || die "missing source onRamp address for CCV configuration"
    [[ -n "$src_offramp" ]] || die "missing source offRamp address for CCV configuration"
    [[ -n "$dst_onramp" ]] || die "missing destination onRamp address for CCV configuration"
    [[ -n "$dst_offramp" ]] || die "missing destination offRamp address for CCV configuration"

    if ! cast call "$src_onramp" "nonce()(uint64)" --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        echo "ERROR: source onRamp at $src_onramp is not reachable or not Symbiotic CCV mock-compatible" >&2
        echo "Redeploy CCV contracts with: make deploy-ccv-contracts" >&2
        exit 1
    fi
    if ! cast call "$dst_offramp" "sourceChainSelector()(uint64)" --rpc-url http://localhost:8546 >/dev/null 2>&1; then
        echo "ERROR: destination offRamp at $dst_offramp is not Symbiotic CCV mock-compatible" >&2
        echo "Redeploy CCV contracts with: make deploy-ccv-contracts" >&2
        exit 1
    fi
    if ! cast call "$dst_onramp" "nonce()(uint64)" --rpc-url http://localhost:8546 >/dev/null 2>&1; then
        echo "ERROR: destination onRamp at $dst_onramp is not reachable or not Symbiotic CCV mock-compatible" >&2
        echo "Redeploy CCV contracts with: make deploy-ccv-contracts" >&2
        exit 1
    fi
    if ! cast call "$src_offramp" "sourceChainSelector()(uint64)" --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        echo "ERROR: source offRamp at $src_offramp is not Symbiotic CCV mock-compatible" >&2
        echo "Redeploy CCV contracts with: make deploy-ccv-contracts" >&2
        exit 1
    fi

    echo "Configuring source SymbioticCCV ($src_ccv) for remote selector $dst_selector..."
    (
        cd "$PROJECT_ROOT/contracts"
        CCV_REMOTE_CHAIN_SELECTOR="$dst_selector" \
        CCV_ONRAMP_ADDRESS="$src_onramp" \
        CCV_OFFRAMP_ADDRESS="$src_offramp" \
        forge script script/ConfigureCCV.s.sol:ConfigureCCV \
            --sig "run(address)" "$src_ccv" \
            --rpc-url http://localhost:8545 \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet

        echo "Configuring destination SymbioticCCV ($dst_ccv) for remote selector $src_selector..."
        CCV_REMOTE_CHAIN_SELECTOR="$src_selector" \
        CCV_ONRAMP_ADDRESS="$dst_onramp" \
        CCV_OFFRAMP_ADDRESS="$dst_offramp" \
        forge script script/ConfigureCCV.s.sol:ConfigureCCV \
            --sig "run(address)" "$dst_ccv" \
            --rpc-url http://localhost:8546 \
            --broadcast \
            --private-key "$PRIVATE_KEY" \
            --quiet
    )

    echo "✓ SymbioticCCV remote-chain config applied"
}

main "$@"
