#!/usr/bin/env bash
# Quick status check for a message across all operators
#
# Usage:
#   ./scripts/msg-status.sh                    # Check last sent message
#   ./scripts/msg-status.sh 0x123...           # Check specific GUID
#   GUID=0x123 ./scripts/msg-status.sh         # Via env var

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_FILE="$PROJECT_ROOT/.cache/last-message.json"

GUID="${GUID:-${1:-}}"

# Load from cache if no GUID specified
if [[ -z "$GUID" ]]; then
    if [[ -f "$CACHE_FILE" ]]; then
        GUID=$(jq -r '.guid // empty' "$CACHE_FILE" 2>/dev/null || true)
        TX=$(jq -r '.tx_hash // empty' "$CACHE_FILE" 2>/dev/null || true)
        MSG=$(jq -r '.message // empty' "$CACHE_FILE" 2>/dev/null || true)
        echo "Last message: \"$MSG\""
        echo "TX: $TX"
        echo ""
    fi
fi

if [[ -z "$GUID" || "$GUID" == "null" ]]; then
    echo "No GUID available. Checking by recent messages..."
    echo ""
fi

echo "═══════════════════════════════════════════════════════════════════"
echo "Operator Status"
echo "═══════════════════════════════════════════════════════════════════"

for i in 1 2 3; do
    port=$((3000 + i))
    echo ""
    echo "operator-$i (port $port):"

    # Always use list endpoint to get status (individual endpoint doesn't include it)
    all_messages=$(curl -sf "http://localhost:$port/debug/v1/messages?limit=50" 2>/dev/null || echo "{}")

    if [[ -n "$GUID" && "$GUID" != "null" ]]; then
        response=$(echo "$all_messages" | jq --arg id "$GUID" '.messages[]? | select(.metadata.message_id == $id)' 2>/dev/null || echo "")
    else
        response=$(echo "$all_messages" | jq '.messages[0]? // empty' 2>/dev/null || echo "")
    fi

    if [[ -z "$response" || "$response" == "{}" || "$response" == "null" ]]; then
        echo "  Status: No message found"
        continue
    fi

    # Extract fields
    msg_id=$(echo "$response" | jq -r '.metadata.message_id // "?"' 2>/dev/null)
    status=$(echo "$response" | jq -r '.status // "?"' 2>/dev/null)
    src_chain=$(echo "$response" | jq -r '.metadata.source_chain // "?"' 2>/dev/null)
    dst_chain=$(echo "$response" | jq -r '.metadata.destination_chain // "?"' 2>/dev/null)
    sub_state=$(echo "$response" | jq -r '.submission.state // "Pending"' 2>/dev/null)
    sub_tx=$(echo "$response" | jq -r '.submission.tx_hash // empty' 2>/dev/null)
    relayer_id=$(echo "$response" | jq -r '.submission.relayer_tx_id // empty' 2>/dev/null)

    echo "  GUID: ${msg_id:0:20}..."
    echo "  Route: $src_chain → $dst_chain"
    echo "  Status: $status"
    echo "  Submission: $sub_state"
    [[ -n "$sub_tx" && "$sub_tx" != "null" ]] && echo "  Dest TX: $sub_tx"
    [[ -n "$relayer_id" && "$relayer_id" != "null" ]] && echo "  Relayer ID: $relayer_id"
done

echo ""
echo "═══════════════════════════════════════════════════════════════════"
echo "Pending Merkle Roots"
echo "═══════════════════════════════════════════════════════════════════"

pending=$(curl -sf "http://localhost:3001/debug/v1/pending" 2>/dev/null || echo "[]")
pending_count=$(echo "$pending" | jq 'length' 2>/dev/null || echo "0")

if [[ "$pending_count" == "0" || "$pending_count" == "null" ]]; then
    echo "None (all signatures collected)"
else
    echo "$pending_count pending root(s) awaiting signatures"
fi
echo ""
