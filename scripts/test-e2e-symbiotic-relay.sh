#!/bin/bash
# E2E Test: Send message via TestOApp, verify DVN proof submission on dest chain
#
# This template provides a custom DVN (Decentralized Verifier Network).
# The test passes when the DVN successfully verifies the cross-chain message.
#
# Note: Message delivery to the dest OApp requires an Executor (not included).
# In production, LayerZero provides the default Executor service.

set -euo pipefail

DEPLOY_DATA="$(dirname "$0")/../data/deploy-data"
SOURCE_RPC="http://localhost:8545"
DEST_RPC="http://localhost:8546"
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
TIMEOUT="${1:-120}"
DEST_EID=31338

# Capture starting block for event queries
START_BLOCK=$(cast block-number --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")

# Load contract addresses
TEST_OAPP=$(jq -r '.testOApp' "$DEPLOY_DATA/testoapp_source.json")
DEST_DVN=$(jq -r '.dvn' "$DEPLOY_DATA/dest_contracts.json")

echo "=== E2E Test: DVN Verification ==="
echo "TestOApp (source): $TEST_OAPP"
echo "DVN (dest):        $DEST_DVN"
echo ""

# Build options (200k gas for lzReceive)
echo "Building options..."
OPTIONS=$(cast call "$TEST_OAPP" "buildOptions(uint128)(bytes)" 200000 --rpc-url "$SOURCE_RPC")

# Quote the fee
echo "Getting quote..."
QUOTE_RESULT=$(cast call "$TEST_OAPP" "quote(uint32,string,bytes,bool)((uint256,uint256))" $DEST_EID "Hello from e2e test" "$OPTIONS" false --rpc-url "$SOURCE_RPC")
FEE=$(echo "$QUOTE_RESULT" | tr -d '()' | cut -d',' -f1 | tr -d ' ' | cut -d'[' -f1)
# Validate FEE is numeric
if ! [[ "$FEE" =~ ^[0-9]+$ ]] || [ "$FEE" = "0" ]; then
    echo "Warning: Could not parse fee from quote, using default"
    FEE="1000000000000000"
fi
echo "Fee (wei): $FEE"

# Send test message via TestOApp
echo ""
echo "Sending message via TestOApp..."
TX=$(cast send "$TEST_OAPP" \
    "send(uint32,string,bytes)" \
    $DEST_EID \
    "Hello from e2e test" \
    "$OPTIONS" \
    --value "$FEE" \
    --rpc-url "$SOURCE_RPC" \
    --private-key "$PRIVATE_KEY" \
    --json | jq -r '.transactionHash')
echo "TX: $TX"

# Verify message was sent
MESSAGES_SENT=$(cast call "$TEST_OAPP" "messagesSent()(uint256)" --rpc-url "$SOURCE_RPC" | cut -d'[' -f1 | tr -d ' ')
echo "Messages sent: $MESSAGES_SENT"

# Operator API endpoint
OPERATOR_API="http://localhost:3001"

# Track pipeline stages
STAGE_JOB_RECEIVED=false
STAGE_SIGNATURES_COLLECTED=false
STAGE_DVN_VERIFIED=false

echo ""
echo "Waiting for DVN verification (timeout: ${TIMEOUT}s)..."
echo "Pipeline: [Source OApp] → [Operator] → [Signatures] → [DVN Verify]"
echo ""

for i in $(seq 1 $((TIMEOUT / 2))); do
    # Check operator job status via debug API
    if [ "$STAGE_JOB_RECEIVED" = "false" ] || [ "$STAGE_SIGNATURES_COLLECTED" = "false" ]; then
        JOB_RESPONSE=$(curl -s "$OPERATOR_API/debug/v1/messages" 2>/dev/null || echo "{}")

        if [ -n "$JOB_RESPONSE" ] && [ "$JOB_RESPONSE" != "{}" ]; then
            JOB_COUNT=$(echo "$JOB_RESPONSE" | jq '.messages | length' 2>/dev/null || echo "0")

            if [ "$JOB_COUNT" != "0" ] && [ "$STAGE_JOB_RECEIVED" = "false" ]; then
                STAGE_JOB_RECEIVED=true
                echo "[$(date +%H:%M:%S)] ✓ Operator: Job received"
            fi

            # Check for signature collection via pending merkle roots
            # When a merkle root gets its proof attached, it's removed from pending
            if [ "$STAGE_SIGNATURES_COLLECTED" = "false" ]; then
                PENDING_RESPONSE=$(curl -s "http://localhost:3001/debug/v1/pending" 2>/dev/null || echo "[]")
                PENDING_COUNT=$(echo "$PENDING_RESPONSE" | jq 'length' 2>/dev/null || echo "0")

                if [ "$PENDING_COUNT" = "0" ] || [ "$PENDING_COUNT" = "null" ]; then
                    # No pending = all proofs attached (signatures collected)
                    STAGE_SIGNATURES_COLLECTED=true
                    echo "[$(date +%H:%M:%S)] ✓ Signatures: collected (no pending merkle roots)"
                fi
            fi
        fi
    fi

    # Check DVN verification - this is the success condition
    if [ "$STAGE_DVN_VERIFIED" = "false" ]; then
        DVN_EVENTS=$(cast logs --from-block "$START_BLOCK" --address "$DEST_DVN" --rpc-url "$DEST_RPC" 2>/dev/null | head -1)
        if [ -n "$DVN_EVENTS" ]; then
            STAGE_DVN_VERIFIED=true
            echo "[$(date +%H:%M:%S)] ✓ DVN: Proof verified on dest chain"
            echo ""
            echo "=== PASSED ==="
            echo "DVN successfully verified the cross-chain message."
            echo ""
            echo "Note: Message delivery to dest OApp requires an Executor."
            echo "      In production, LayerZero provides the default Executor."
            exit 0
        fi
    fi

    # Progress indicator every 10 seconds
    if [ $((i % 5)) -eq 0 ]; then
        STATUS_LINE="job:$([ "$STAGE_JOB_RECEIVED" = "true" ] && echo "✓" || echo "?")"
        STATUS_LINE="$STATUS_LINE sigs:$([ "$STAGE_SIGNATURES_COLLECTED" = "true" ] && echo "✓" || echo "?")"
        STATUS_LINE="$STATUS_LINE dvn:$([ "$STAGE_DVN_VERIFIED" = "true" ] && echo "✓" || echo "?")"
        echo "[$(date +%H:%M:%S)] Waiting... ($STATUS_LINE)"
    fi

    sleep 2
done

echo ""
echo "=== FAILED ==="
echo "Timeout after ${TIMEOUT}s"
echo ""
echo "Pipeline status:"
echo "  [$([ "$STAGE_JOB_RECEIVED" = "true" ] && echo "✓" || echo "✗")] Job received by operator"
echo "  [$([ "$STAGE_SIGNATURES_COLLECTED" = "true" ] && echo "✓" || echo "✗")] Signatures collected"
echo "  [$([ "$STAGE_DVN_VERIFIED" = "true" ] && echo "✓" || echo "✗")] DVN proof verified"
echo ""

# Identify likely failure point
if [ "$STAGE_JOB_RECEIVED" = "false" ]; then
    echo "Likely issue: OZ Monitor not detecting events or operator not receiving webhooks"
    echo "  Check: docker logs oz-monitor --tail 50"
    echo "  Check: docker logs operator-1 --tail 50 | grep webhook"
elif [ "$STAGE_SIGNATURES_COLLECTED" = "false" ]; then
    echo "Likely issue: Symbiotic relay sidecars not responding"
    echo "  Check: docker logs symbiotic-relay-1 --tail 50"
    echo "  Check: curl -s http://localhost:8081/healthz"
else
    echo "Likely issue: OZ Relayer not submitting proof transaction"
    echo "  Check: docker logs oz-relayer --tail 50"
    echo "  Check: docker logs operator-1 --tail 50 | grep -i relayer"
fi
echo ""
echo "Full debug:"
echo "  curl -s $OPERATOR_API/debug/v1/messages | jq '.messages'"
exit 1
