#!/usr/bin/env bash
# Send a test message and save the GUID for tracking
#
# Usage:
#   ./scripts/send-message.sh                    # Send "hello"
#   ./scripts/send-message.sh "custom message"  # Send custom message
#   MSG="test" ./scripts/send-message.sh        # Via env var
#
# Output: Prints TX hash and GUID, saves to .cache/last-message.json

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_DIR="$PROJECT_ROOT/.cache"
DEPLOY_DATA="$PROJECT_ROOT/data/deploy-data"

# Load addresses if available
if [[ -f "$DEPLOY_DATA/addresses.env" ]]; then
    set -a
    source "$DEPLOY_DATA/addresses.env"
    set +a
fi

# Defaults
SOURCE_RPC="${SOURCE_RPC_URL:-http://localhost:8545}"
DEST_EID="${DEST_CHAIN_ID:-31338}"
PRIVATE_KEY="${PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
GAS_LIMIT="${GAS:-200000}"
MSG="${MSG:-${1:-hello}}"

# Get TestOApp address
if [[ -z "${TEST_OAPP_SOURCE_ADDRESS:-}" ]]; then
    if [[ -f "$DEPLOY_DATA/testoapp_source.json" ]]; then
        TEST_OAPP_SOURCE_ADDRESS=$(jq -r '.testOApp' "$DEPLOY_DATA/testoapp_source.json")
    else
        echo "ERROR: No TestOApp address. Run 'make start' first." >&2
        exit 1
    fi
fi

echo "Sending message: \"$MSG\""
echo "  To: EID $DEST_EID"
echo "  Gas: $GAS_LIMIT"
echo ""

# Build options
OPTIONS=$(cast call "$TEST_OAPP_SOURCE_ADDRESS" "buildOptions(uint128)(bytes)" "$GAS_LIMIT" --rpc-url "$SOURCE_RPC")

# Quote the fee
QUOTE_RESULT=$(cast call "$TEST_OAPP_SOURCE_ADDRESS" "quote(uint32,string,bytes,bool)((uint256,uint256))" "$DEST_EID" "$MSG" "$OPTIONS" false --rpc-url "$SOURCE_RPC")
FEE=$(echo "$QUOTE_RESULT" | tr -d '()' | cut -d',' -f1 | tr -d ' ' | cut -d'[' -f1)

# Validate fee
if ! [[ "$FEE" =~ ^[0-9]+$ ]] || [[ "$FEE" == "0" ]]; then
    FEE="1000000000000000"  # 0.001 ETH fallback
fi

FEE_ETH=$(echo "scale=6; $FEE / 1000000000000000000" | bc 2>/dev/null || echo "~0.001")
echo "Fee: $FEE wei (~$FEE_ETH ETH)"

# Send the message
echo "Sending..."
TX_JSON=$(cast send "$TEST_OAPP_SOURCE_ADDRESS" \
    "send(uint32,string,bytes)" \
    "$DEST_EID" \
    "$MSG" \
    "$OPTIONS" \
    --value "$FEE" \
    --rpc-url "$SOURCE_RPC" \
    --private-key "$PRIVATE_KEY" \
    --json)

TX_HASH=$(echo "$TX_JSON" | jq -r '.transactionHash')
BLOCK_HEX=$(echo "$TX_JSON" | jq -r '.blockNumber')
# Convert hex to decimal
BLOCK=$((BLOCK_HEX))

echo ""
echo "TX: $TX_HASH"
echo "Block: $BLOCK"

# Wait a moment for operators to receive the event
echo ""
echo "Waiting for operators to receive event..."
sleep 3

# Try to find the message GUID from operators
GUID=""
for port in 3001 3002 3003; do
    RESPONSE=$(curl -sf "http://localhost:$port/debug/v1/messages?limit=10" 2>/dev/null || echo "{}")
    if [[ "$RESPONSE" != "{}" ]]; then
        # Find message matching our tx hash
        FOUND_GUID=$(echo "$RESPONSE" | jq -r --arg tx "$TX_HASH" \
            '.messages[]? | select(.metadata.event_tx_hash == $tx) | .metadata.message_id' 2>/dev/null | head -1)
        if [[ -n "$FOUND_GUID" && "$FOUND_GUID" != "null" ]]; then
            GUID="$FOUND_GUID"
            break
        fi
    fi
done

# Save to cache
mkdir -p "$CACHE_DIR"

# Format GUID as JSON (null if empty, quoted string if set)
if [[ -n "$GUID" ]]; then
    GUID_JSON="\"$GUID\""
else
    GUID_JSON="null"
fi

cat > "$CACHE_DIR/last-message.json" <<EOF
{
  "tx_hash": "$TX_HASH",
  "block": $BLOCK,
  "guid": $GUID_JSON,
  "message": "$MSG",
  "dest_eid": $DEST_EID,
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

echo ""
if [[ -n "$GUID" ]]; then
    echo "GUID: $GUID"
    echo ""
    echo "Track with: make watch"
    echo "Or:         make watch GUID=$GUID"
else
    echo "GUID: (not yet available - operators may still be processing)"
    echo ""
    echo "Track with: make watch TX=$TX_HASH"
fi

echo ""
echo "Saved to .cache/last-message.json"
