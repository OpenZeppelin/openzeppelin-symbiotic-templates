#!/usr/bin/env bash
# Chainlink CCV provider logic for scripts/msg (sourced file)

cmd_send_chainlink_ccv() {
    load_addresses || true

    local onramp
    onramp="$(get_ccv_source_onramp_address 2>/dev/null || true)"
    if [[ -z "$onramp" ]]; then
        die "unable to resolve CCV source onRamp address. Set providers.chainlink_ccv.source_onramp_address (or deploy CCV artifacts) and run make configure"
    fi
    if ! [[ "$onramp" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        die "invalid source onRamp address: $onramp"
    fi

    local offramp
    offramp="$(get_ccv_dest_offramp_address 2>/dev/null || true)"
    if [[ -z "$offramp" ]]; then
        die "unable to resolve CCV destination offRamp address. Set providers.chainlink_ccv.destination_offramp_address (or deploy CCV artifacts) and run make configure"
    fi
    if ! [[ "$offramp" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        die "invalid destination offRamp address: $offramp"
    fi

    local dest_selector
    dest_selector="$(get_ccv_dest_chain_selector)"
    if ! [[ "$dest_selector" =~ ^[0-9]+$ ]]; then
        die "invalid destination chain selector: $dest_selector"
    fi

    local topic0
    topic0="$(cast keccak "$CCV_EVENT_SIGNATURE")"

    local encoded_message
    encoded_message="$(cast abi-encode "f(string)" "$MESSAGE")"

    if $DRY_RUN; then
        echo "# CCV send emits a real on-chain CCIPMessageSent event via OnRamp-compatible entrypoint"
        echo "cast send $onramp \"sendMessage(uint64,bytes,bytes4)\" $dest_selector $encoded_message $CCV_VERSION_TAG --rpc-url $SOURCE_RPC --private-key $PRIVATE_KEY --json"
        return 0
    fi

    if ! cast client --rpc-url "$SOURCE_RPC" >/dev/null 2>&1; then
        die "source RPC not reachable at $SOURCE_RPC"
    fi
    if ! cast client --rpc-url "$DEST_RPC" >/dev/null 2>&1; then
        die "destination RPC not reachable at $DEST_RPC"
    fi
    if ! cast call "$offramp" "sourceChainSelector()(uint64)" --rpc-url "$DEST_RPC" >/dev/null 2>&1; then
        die "destination offRamp $offramp is not reachable or not SymbioticCCV-compatible"
    fi

    local tx_json tx_hash receipt_json message_id block_hex block_number
    tx_json="$(cast send "$onramp" \
        "sendMessage(uint64,bytes,bytes4)" \
        "$dest_selector" \
        "$encoded_message" \
        "$CCV_VERSION_TAG" \
        --rpc-url "$SOURCE_RPC" \
        --private-key "$PRIVATE_KEY" \
        --json)"
    tx_hash="$(echo "$tx_json" | jq -r '.transactionHash')"
    receipt_json="$(cast receipt "$tx_hash" --rpc-url "$SOURCE_RPC" --json)"
    message_id="$(echo "$receipt_json" | jq -r --arg topic "$topic0" '(.logs // [])[]? | select((.topics[0] // "" | ascii_downcase) == ($topic | ascii_downcase)) | .topics[3] // empty' | head -n 1)"
    block_hex="$(echo "$receipt_json" | jq -r '.blockNumber // empty')"

    if [[ -z "$message_id" ]]; then
        die "unable to locate CCIPMessageSent log for tx $tx_hash on onRamp $onramp"
    fi

    if [[ -n "$block_hex" ]]; then
        block_number=$((block_hex))
    else
        block_number="$(cast block-number --rpc-url "$SOURCE_RPC" 2>/dev/null || echo "0")"
    fi

    if ! $JSON_OUTPUT; then
        echo "Provider: chainlink_ccv"
        echo "Sending on-chain CCIP message via OnRamp-compatible entrypoint"
        echo "Mode: ${CCV_MODE:-symbiotic_mock}"
        echo "Message ID: $message_id"
        echo "Source OnRamp: $onramp"
        echo "Dest selector: $dest_selector"
        echo "TX: $tx_hash"
    fi

    save_to_cache "$tx_hash" "$block_number" "$message_id" "$MESSAGE" "$dest_selector"

    if $JSON_OUTPUT; then
        echo "{\"provider\":\"chainlink_ccv\",\"mode\":\"onchain_send\",\"message_id\":\"$message_id\",\"tx_hash\":\"$tx_hash\",\"block\":$block_number}"
    else
        echo ""
        echo "CCV source tx submitted. Track with:"
        echo "  make watch"
    fi
}

ccv_watch_print_banner() {
    if $JSON_OUTPUT; then
        return 0
    fi

    echo "═══════════════════════════════════════════════════════════════════"
    echo "Watching Chainlink CCV message (timeout: ${TIMEOUT}s)"
    echo "═══════════════════════════════════════════════════════════════════"
    echo ""
}

ccv_watch_collect_operator_state() {
    local found_guid=""
    loop_best_status=""
    loop_best_submission="Pending"
    loop_best_submission_error=""
    loop_tx_hash_dest=""
    loop_ccv_onchain_verified=false
    loop_ccv_onchain_tx=""

    for i in 1 2 3; do
        local port=$((3000 + i))
        local response
        response=$(query_operator "$port" "$GUID" "$TX_HASH")

        if [[ "$response" != "{}" && -n "$response" && "$response" != "null" ]]; then
            if [[ -z "$GUID" || "$GUID" == "null" ]]; then
                found_guid=$(echo "$response" | jq -r '.metadata.message_id // empty' 2>/dev/null || true)
                [[ -n "$found_guid" && "$found_guid" != "null" ]] && GUID="$found_guid"
            fi

            local status submission_state submission_tx submission_error
            status=$(echo "$response" | jq -r '.status // "?"' 2>/dev/null || echo "?")
            submission_state=$(echo "$response" | jq -r '.submission.state // "Pending"' 2>/dev/null || echo "Pending")
            submission_tx=$(echo "$response" | jq -r '.submission.tx_hash // empty' 2>/dev/null || true)
            submission_error=$(echo "$response" | jq -r '.submission.last_error // empty' 2>/dev/null || true)

            case $status in
                Signed)
                    loop_best_status="Signed"
                    ;;
                Processing) [[ "$loop_best_status" != "Signed" ]] && loop_best_status="Processing" ;;
                Pending) [[ -z "$loop_best_status" ]] && loop_best_status="Pending" ;;
                *) [[ -z "$loop_best_status" ]] && loop_best_status="$status" ;;
            esac

            case "$submission_state" in
                Confirmed)
                    loop_best_submission="Confirmed"
                    loop_tx_hash_dest="$submission_tx"
                    ;;
                Submitted)
                    if [[ "$loop_best_submission" != "Confirmed" ]]; then
                        loop_best_submission="Submitted"
                        loop_tx_hash_dest="$submission_tx"
                    fi
                    ;;
                Failed)
                    if [[ "$loop_best_submission" != "Confirmed" && "$loop_best_submission" != "Submitted" ]]; then
                        loop_best_submission="Failed"
                        loop_tx_hash_dest="$submission_tx"
                        if [[ -n "$submission_error" && "$submission_error" != "null" ]]; then
                            loop_best_submission_error="$submission_error"
                        fi
                    fi
                    ;;
            esac
        fi
    done
}

ccv_watch_maybe_verify_onchain() {
    if [[ "$loop_best_submission" != "Confirmed" || -z "$loop_tx_hash_dest" || "$loop_tx_hash_dest" == "null" || -z "$GUID" || "$GUID" == "null" ]]; then
        return 0
    fi

    if [[ -z "$ccv_executed_topic" ]]; then
        ccv_executed_topic="$(cast keccak "$CCV_EXECUTED_EVENT_SIGNATURE" 2>/dev/null || true)"
    fi

    local expected_topic receipt_json tx_status match_found
    expected_topic="$(echo "$ccv_executed_topic" | tr '[:upper:]' '[:lower:]')"
    receipt_json="$(cast receipt "$loop_tx_hash_dest" --rpc-url "$DEST_RPC" --json 2>/dev/null || true)"

    if [[ -z "$receipt_json" || "$receipt_json" == "null" ]]; then
        return 0
    fi

    tx_status="$(echo "$receipt_json" | jq -r '.status // empty' 2>/dev/null || true)"
    if [[ "$tx_status" != "0x1" && "$tx_status" != "1" ]]; then
        return 0
    fi

    match_found="$(
        echo "$receipt_json" | jq -r \
            --arg topic "$expected_topic" \
            --arg guid "$(echo "$GUID" | tr '[:upper:]' '[:lower:]')" \
            '
            (.logs // [])[]?
            | select((.topics[0] // "" | ascii_downcase) == $topic)
            | select((.topics[1] // "" | ascii_downcase) == $guid)
            | "matched"
            ' 2>/dev/null | head -n 1
    )"

    if [[ -n "$match_found" ]]; then
        loop_ccv_onchain_verified=true
        loop_ccv_onchain_tx="$loop_tx_hash_dest"
    fi
}

ccv_watch_print_progress_update() {
    local current_status="${best_status}:${best_submission}:${ccv_onchain_verified}"
    if [[ "$current_status" != "$last_status" ]] && ! $JSON_OUTPUT; then
        local timestamp prev_status prev_submission prev_onchain
        timestamp=$(date +%H:%M:%S)
        IFS=':' read -r prev_status prev_submission prev_onchain <<< "$last_status"

        if [[ -n "$GUID" && "$last_status" == "" ]]; then
            echo "[$timestamp] Message ID: $GUID"
        fi
        [[ "$best_status" != "$prev_status" && -n "$best_status" ]] && echo "[$timestamp] $(format_status "$best_status")"
        if [[ "$best_submission" != "$prev_submission" && -n "$best_submission" && "$best_submission" != "Pending" ]]; then
            echo "[$timestamp] $(format_relayer_status "$best_submission" "$tx_hash_dest")"
        fi
        if [[ "$best_submission" == "Confirmed" && "$ccv_onchain_verified" != "true" && "$prev_onchain" != "true" ]]; then
            echo "[$timestamp] Destination: waiting for OffRamp MessageExecuted log"
        fi
        if [[ "$ccv_onchain_verified" == "true" && "$prev_onchain" != "true" ]]; then
            if [[ -n "$ccv_onchain_tx" ]]; then
                echo "[$timestamp] Destination: MessageExecuted confirmed (tx: $ccv_onchain_tx)"
            else
                echo "[$timestamp] Destination: MessageExecuted confirmed"
            fi
        fi

        last_status="$current_status"
    fi
}

ccv_watch_exit_timeout() {
    local elapsed="$1"
    if $JSON_OUTPUT; then
        echo "{\"status\":\"timeout\",\"stage\":\"${best_status:-unknown}\",\"submission_state\":\"${best_submission:-Pending}\",\"elapsed\":$elapsed}"
    else
        echo ""
        echo "Timed out after ${TIMEOUT}s waiting for Chainlink CCV destination confirmation"
        echo "Last stage: operators=${best_status:-unknown}, relayer=${best_submission:-Pending}"
        if [[ "$best_submission" == "Submitted" ]]; then
            echo "Tip: relayer submitted, but destination execution is still pending."
            echo "Tip: check relayer/operator logs with 'make logs-relayer' and 'make logs-operators'"
        elif [[ "$best_submission" == "Confirmed" && "$ccv_onchain_verified" != "true" ]]; then
            echo "Tip: relayer shows confirmed, but destination MessageExecuted log was not found."
            echo "Tip: verify destination tx receipt and offRamp events."
        elif [[ "$best_submission" == "Failed" && -n "$best_submission_error" ]]; then
            echo "Last relayer error: $best_submission_error"
            if echo "$best_submission_error" | grep -q "0xf5ab0d81"; then
                echo "Hint: 0xf5ab0d81 maps to EpochTooStale() in SymbioticCCV."
                echo "Hint: run 'make clean && make setup && make start' to refresh local settlement state."
            fi
        else
            echo "Tip: check relay sidecars and operator batching progress."
        fi
    fi
    exit 1
}

ccv_watch_exit_failed() {
    local elapsed="$1"
    if $JSON_OUTPUT; then
        echo "{\"status\":\"failed\",\"stage\":\"$best_status\",\"message_id\":\"$GUID\",\"submission_state\":\"Failed\",\"elapsed\":$elapsed}"
    else
        echo ""
        echo "CCV payload submission failed"
        [[ -n "$GUID" ]] && echo "Message ID: $GUID"
        echo "Submission state: Failed"
        if [[ -n "$best_submission_error" ]]; then
            echo "Relayer error: $best_submission_error"
            if echo "$best_submission_error" | grep -q "0xf5ab0d81"; then
                echo "Hint: 0xf5ab0d81 maps to EpochTooStale() in SymbioticCCV."
                echo "Hint: run 'make clean && make setup && make start' to refresh local settlement state."
            fi
        fi
        echo "Tip: inspect operator + relayer logs for revert details"
    fi
    exit 1
}

ccv_watch_exit_confirmed() {
    local elapsed="$1"
    if $JSON_OUTPUT; then
        echo "{\"status\":\"confirmed\",\"stage\":\"$best_status\",\"message_id\":\"$GUID\",\"submission_state\":\"$best_submission\",\"dest_tx\":\"$ccv_onchain_tx\",\"elapsed\":$elapsed}"
    else
        echo ""
        echo "CCV payload verified on destination chain"
        [[ -n "$GUID" ]] && echo "Message ID: $GUID"
        echo "Submission state: Confirmed"
        [[ -n "$ccv_onchain_tx" && "$ccv_onchain_tx" != "null" ]] && echo "Dest TX: $ccv_onchain_tx"
        echo "Verifier path: SymbioticCCV.verifyMessage executed via OffRamp mock"
    fi
    exit 0
}

cmd_watch_chainlink_ccv() {
    if $DRY_RUN; then
        echo "# Poll operators for CCV message status"
        echo "curl -s http://localhost:3001/debug/v1/messages?limit=50"
        return 0
    fi

    load_watch_target_from_cache

    ccv_watch_print_banner

    local start_time last_status
    local best_status best_submission best_submission_error tx_hash_dest ccv_onchain_verified ccv_onchain_tx ccv_executed_topic
    local loop_best_status loop_best_submission loop_best_submission_error loop_tx_hash_dest
    local loop_ccv_onchain_verified loop_ccv_onchain_tx
    start_time=$(date +%s)
    last_status=""
    best_status="Pending"
    best_submission="Pending"
    best_submission_error=""
    tx_hash_dest=""
    ccv_onchain_verified=false
    ccv_onchain_tx=""
    ccv_executed_topic=""

    while true; do
        local elapsed=$(( $(date +%s) - start_time ))
        if [[ $elapsed -ge $TIMEOUT ]]; then
            ccv_watch_exit_timeout "$elapsed"
        fi

        ccv_watch_collect_operator_state
        ccv_watch_maybe_verify_onchain

        best_status="$loop_best_status"
        best_submission="$loop_best_submission"
        best_submission_error="$loop_best_submission_error"
        tx_hash_dest="$loop_tx_hash_dest"
        ccv_onchain_verified="$loop_ccv_onchain_verified"
        ccv_onchain_tx="$loop_ccv_onchain_tx"

        ccv_watch_print_progress_update

        if [[ "$best_submission" == "Failed" ]]; then
            ccv_watch_exit_failed "$elapsed"
        fi

        # For CCV flow, success requires on-chain OffRamp MessageExecuted confirmation.
        if [[ "$ccv_onchain_verified" == "true" ]]; then
            ccv_watch_exit_confirmed "$elapsed"
        fi

        sleep 2
    done
}
