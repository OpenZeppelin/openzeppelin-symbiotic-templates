#!/bin/bash
# E2E Test: Send message on source chain, verify proof submitted on dest chain
set -euo pipefail

DEPLOY_DATA="$(dirname "$0")/../data/deploy-data"
SOURCE_RPC="http://localhost:8545"
DEST_RPC="http://localhost:8546"
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
TIMEOUT="${1:-120}"

# Load contract addresses
MOCK_SEND_ULN=$(jq -r '.sendUln' "$DEPLOY_DATA/source_contracts.json")
MOCK_RECEIVE_ULN=$(jq -r '.receiveUln' "$DEPLOY_DATA/dest_contracts.json")

echo "=== E2E Test ==="
echo "MockSendUln:    $MOCK_SEND_ULN"
echo "MockReceiveUln: $MOCK_RECEIVE_ULN"

# Get initial verification count
INITIAL=$(cast call "$MOCK_RECEIVE_ULN" "verificationCount()(uint256)" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")
echo "Initial verificationCount: $INITIAL"

# Send test message
echo ""
echo "Sending message..."
TX=$(cast send "$MOCK_SEND_ULN" \
    "sendMessage(uint32,bytes32,bytes,bytes)" \
    31338 \
    "0x0000000000000000000000000000000000000000000000000000000000001234" \
    "0x48656c6c6f" \
    "0x" \
    --rpc-url "$SOURCE_RPC" \
    --private-key "$PRIVATE_KEY" \
    --json | jq -r '.transactionHash')
echo "TX: $TX"

# Poll for verification
echo ""
echo "Waiting for verification (timeout: ${TIMEOUT}s)..."
for i in $(seq 1 $((TIMEOUT / 5))); do
    CURRENT=$(cast call "$MOCK_RECEIVE_ULN" "verificationCount()(uint256)" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")
    if [ "$CURRENT" != "$INITIAL" ]; then
        echo ""
        echo "=== PASSED ==="
        echo "verificationCount: $INITIAL -> $CURRENT"
        exit 0
    fi
    printf "."
    sleep 5
done

echo ""
echo "=== FAILED ==="
echo "Timeout: verificationCount still $INITIAL"
echo ""
echo "Debug:"
echo "  docker logs operator --tail 50"
echo "  docker logs oz-relayer --tail 50"
echo "  curl -s http://localhost:3000/debug/v1/messages | jq"
exit 1
