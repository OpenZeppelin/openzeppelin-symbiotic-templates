#!/usr/bin/env bash
# One-shot helper to sweep residual native balances from the mainnet test
# keystores back to the original funder. Defaults to dry-run; pass `exec` to
# actually broadcast.
#
# Usage:
#   scripts/sweep-mainnet-funds.sh         # dry-run, prints plan
#   scripts/sweep-mainnet-funds.sh exec    # broadcasts txs on mainnet
set -euo pipefail

MODE="${1:-dry}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck source=/dev/null
source "${REPO_ROOT}/.env.mainnet"
set +a

: "${KEYSTORE_PASSPHRASE:?KEYSTORE_PASSPHRASE not set in .env.mainnet}"
: "${SWEEP_DEST:?SWEEP_DEST not set in .env.mainnet}"
DEST="${SWEEP_DEST}"

ROLES=(deployer operator-1 operator-2 operator-3 signer-1 signer-2 signer-3)

sweep_chain() {
  local label="$1" rpc="$2"
  echo
  echo "=== ${label} (${rpc%%/v2/*}) ==="
  local gas_price
  gas_price=$(cast gas-price --rpc-url "$rpc")
  # Reserve enough for EIP-1559 max_fee_per_gas (cast send default ~2-3x base
  # fee) plus headroom for base-fee swings between estimate and inclusion.
  # 4x base-fee × 21000 gas works for both chains at current prices.
  local gas_cost=$((gas_price * 21000 * 400 / 100))
  printf "gas_price=%s wei  gas_cost(per tx)=%s wei\n" "$gas_price" "$gas_cost"
  for role in "${ROLES[@]}"; do
    local keystore="${REPO_ROOT}/config/keys/mainnet/${role}.json"
    local addr
    addr=$(cast wallet address --keystore "$keystore" --password "$KEYSTORE_PASSPHRASE")
    local bal
    bal=$(cast balance "$addr" --rpc-url "$rpc")
    if [ "$bal" -le "$gas_cost" ]; then
      printf "  %-10s %s  bal=%-20s  SKIP (<= gas)\n" "$role" "$addr" "$bal"
      continue
    fi
    local send=$((bal - gas_cost))
    printf "  %-10s %s  bal=%-20s  send=%-20s\n" "$role" "$addr" "$bal" "$send"
    if [ "$MODE" = "exec" ]; then
      cast send \
        --keystore "$keystore" \
        --password "$KEYSTORE_PASSPHRASE" \
        --rpc-url "$rpc" \
        --value "$send" \
        "$DEST" \
        >/dev/null
      echo "    ok"
    fi
  done
}

echo "destination: $DEST"
echo "mode:        $MODE"
sweep_chain "Ethereum" "$DEST_RPC_URL"
sweep_chain "Base"     "$SOURCE_RPC_URL"
echo
if [ "$MODE" = "dry" ]; then
  echo "Dry-run complete. Re-run with 'exec' to broadcast."
fi
