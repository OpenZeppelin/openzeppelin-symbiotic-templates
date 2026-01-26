#!/usr/bin/env bash
# Watch a message's lifecycle across all operators
#
# Usage:
#   ./scripts/watch-message.sh                    # Watch last sent message
#   ./scripts/watch-message.sh --guid 0x123...    # Watch specific GUID
#   ./scripts/watch-message.sh --tx 0xabc...      # Find by TX hash
#   GUID=0x123 ./scripts/watch-message.sh         # Via env var
#
# Shows unified status across all 3 operators with live updates.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_FILE="$PROJECT_ROOT/.cache/last-message.json"
DEPLOY_DATA="$PROJECT_ROOT/data/deploy-data"

# Load addresses
if [[ -f "$DEPLOY_DATA/addresses.env" ]]; then
    set -a
    source "$DEPLOY_DATA/addresses.env"
    set +a
fi

DEST_RPC="${DEST_RPC_URL:-http://localhost:8546}"
DVN_DEST="${DVN_DEST_ADDRESS:-}"
TIMEOUT="${TIMEOUT:-120}"

# Parse arguments
GUID="${GUID:-}"
TX_HASH="${TX:-}"

while [[ $# -gt 0 ]]; do
    case $1 in
        --guid|-g) GUID="$2"; shift 2 ;;
        --tx|-t) TX_HASH="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) shift ;;
    esac
done

# Load from cache if no GUID/TX specified
if [[ -z "$GUID" && -z "$TX_HASH" ]]; then
    if [[ -f "$CACHE_FILE" ]]; then
        GUID=$(jq -r 'if .guid == null then "" else .guid end' "$CACHE_FILE" 2>/dev/null || true)
        TX_HASH=$(jq -r '.tx_hash // empty' "$CACHE_FILE" 2>/dev/null || true)
        MSG=$(jq -r '.message // empty' "$CACHE_FILE" 2>/dev/null || true)

        if [[ -z "$TX_HASH" && -z "$GUID" ]]; then
            echo "ERROR: No message to watch. Run 'make send' first or specify --guid/--tx" >&2
            exit 1
        fi

        echo "Watching last message: \"$MSG\""
        [[ -n "$TX_HASH" ]] && echo "TX: $TX_HASH"
        [[ -n "$GUID" && "$GUID" != "null" ]] && echo "GUID: $GUID"
        echo ""
    else
        echo "ERROR: No message to watch. Run 'make send' first or specify --guid/--tx" >&2
        exit 1
    fi
fi

# Get DVN address if not set
if [[ -z "$DVN_DEST" && -f "$DEPLOY_DATA/dest_contracts.json" ]]; then
    DVN_DEST=$(jq -r '.dvn' "$DEPLOY_DATA/dest_contracts.json")
fi

# Function to query operator for message (always use list endpoint to get status)
get_message_status() {
    local port=$1
    local guid=$2
    local tx=$3

    # Always query list endpoint (individual message endpoint doesn't include status)
    local response=$(curl -sf "http://localhost:$port/debug/v1/messages?limit=50" 2>/dev/null || echo "{}")

    if [[ -n "$guid" && "$guid" != "null" ]]; then
        # Filter by GUID
        echo "$response" | jq --arg id "$guid" '.messages[]? | select(.metadata.message_id == $id)' 2>/dev/null || echo "{}"
    elif [[ -n "$tx" ]]; then
        # Filter by TX hash
        echo "$response" | jq --arg tx "$tx" '.messages[]? | select(.metadata.event_tx_hash == $tx)' 2>/dev/null || echo "{}"
    else
        # Return most recent message
        echo "$response" | jq '.messages[0]? // {}' 2>/dev/null || echo "{}"
    fi
}

# Normalize status value (handle variations)
normalize_status() {
    local status=$1
    case $status in
        Pending|pending|PENDING) echo "Pending" ;;
        Processing|processing|PROCESSING) echo "Processing" ;;
        Signed|signed|SIGNED) echo "Signed" ;;
        *) echo "$status" ;;
    esac
}

# Function to format status with color
format_status() {
    local status=$1
    case $status in
        Pending)    echo "⏳ Pending" ;;
        Processing) echo "🔄 Processing" ;;
        Signed)     echo "✅ Signed" ;;
        *)          echo "❓ $status" ;;
    esac
}

format_submission() {
    local state=$1
    case $state in
        Pending)    echo "⏳ Pending" ;;
        Submitted)  echo "📤 Submitted" ;;
        Confirmed)  echo "✅ Confirmed" ;;
        Failed)     echo "❌ Failed" ;;
        *)          echo "❓ $state" ;;
    esac
}

# Initial status display
echo "═══════════════════════════════════════════════════════════════════"
echo "Watching message (timeout: ${TIMEOUT}s)"
echo "═══════════════════════════════════════════════════════════════════"
echo ""

START_TIME=$(date +%s)
LAST_STATUS=""
DVN_VERIFIED=false
START_BLOCK=$(cast block-number --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")

while true; do
    ELAPSED=$(($(date +%s) - START_TIME))

    if [[ $ELAPSED -ge $TIMEOUT ]]; then
        echo ""
        echo "⏱️  Timeout after ${TIMEOUT}s"
        exit 1
    fi

    # Query all operators
    STATUS_LINE=""
    FOUND_GUID=""
    BEST_STATUS=""
    BEST_SUBMISSION=""
    TX_HASH_DEST=""

    for i in 1 2 3; do
        port=$((3000 + i))
        response=$(get_message_status $port "$GUID" "$TX_HASH")

        if [[ "$response" != "{}" && -n "$response" && "$response" != "null" ]]; then
            # Extract GUID if we didn't have it
            if [[ -z "$GUID" || "$GUID" == "null" ]]; then
                FOUND_GUID=$(echo "$response" | jq -r '.metadata.message_id // .message_id // empty' 2>/dev/null || true)
                if [[ -n "$FOUND_GUID" && "$FOUND_GUID" != "null" ]]; then
                    GUID="$FOUND_GUID"
                fi
            fi

            # Try different status field locations
            raw_status=$(echo "$response" | jq -r '.status // .processing_status // "?"' 2>/dev/null || echo "?")
            status=$(normalize_status "$raw_status")
            submission_state=$(echo "$response" | jq -r '.submission.state // .submission_state // "Pending"' 2>/dev/null || echo "Pending")
            submission_tx=$(echo "$response" | jq -r '.submission.tx_hash // empty' 2>/dev/null || true)

            STATUS_LINE+="op$i:$status "

            # Track best status (Signed > Processing > Pending)
            case $status in
                Signed) BEST_STATUS="Signed"; BEST_SUBMISSION="$submission_state"; TX_HASH_DEST="$submission_tx" ;;
                Processing) [[ "$BEST_STATUS" != "Signed" ]] && BEST_STATUS="Processing" ;;
                Pending) [[ -z "$BEST_STATUS" ]] && BEST_STATUS="Pending" ;;
                *) [[ -z "$BEST_STATUS" ]] && BEST_STATUS="$status" ;;
            esac
        else
            STATUS_LINE+="op$i:- "
        fi
    done

    # Check DVN verification on dest chain
    if [[ "$DVN_VERIFIED" == "false" && -n "$DVN_DEST" ]]; then
        DVN_EVENTS=$(cast logs --from-block "$START_BLOCK" --address "$DVN_DEST" --rpc-url "$DEST_RPC" 2>/dev/null | head -1 || true)
        if [[ -n "$DVN_EVENTS" ]]; then
            DVN_VERIFIED=true
        fi
    fi

    # Build current status string
    CURRENT_STATUS="[$STATUS_LINE] submission:${BEST_SUBMISSION:-?} dvn:$([ "$DVN_VERIFIED" == "true" ] && echo "✓" || echo "?")"

    # Only print if status changed
    if [[ "$CURRENT_STATUS" != "$LAST_STATUS" ]]; then
        TIMESTAMP=$(date +%H:%M:%S)

        if [[ -n "$GUID" && "$GUID" != "null" && -z "$FOUND_GUID" ]]; then
            echo "[$TIMESTAMP] GUID: $GUID"
            FOUND_GUID="printed"
        fi

        # Print formatted status
        echo -n "[$TIMESTAMP] "

        if [[ -n "$BEST_STATUS" ]]; then
            echo -n "$(format_status "$BEST_STATUS")"

            if [[ "$BEST_STATUS" == "Signed" && -n "$BEST_SUBMISSION" ]]; then
                echo -n " → $(format_submission "$BEST_SUBMISSION")"
                if [[ -n "$TX_HASH_DEST" && "$TX_HASH_DEST" != "null" ]]; then
                    echo -n " (tx: ${TX_HASH_DEST:0:10}...)"
                fi
            fi

            if [[ "$DVN_VERIFIED" == "true" ]]; then
                echo -n " → ✅ DVN Verified"
            fi

            echo ""
        else
            echo "Waiting for operators... ($STATUS_LINE)"
        fi

        LAST_STATUS="$CURRENT_STATUS"
    fi

    # Success condition
    if [[ "$DVN_VERIFIED" == "true" ]]; then
        echo ""
        echo "═══════════════════════════════════════════════════════════════════"
        echo "✅ Message verified on destination chain!"
        echo "═══════════════════════════════════════════════════════════════════"
        [[ -n "$GUID" ]] && echo "GUID: $GUID"
        [[ -n "$TX_HASH_DEST" && "$TX_HASH_DEST" != "null" ]] && echo "Dest TX: $TX_HASH_DEST"
        echo ""
        echo "Note: Message delivery requires an Executor (not included in devnet)"
        exit 0
    fi

    sleep 2
done
