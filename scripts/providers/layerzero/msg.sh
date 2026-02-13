#!/usr/bin/env bash
# LayerZero provider logic for scripts/msg (sourced file)

cmd_send_layerzero() {
    load_addresses || true

    local testoapp
    testoapp=$(get_testoapp_address 2>/dev/null) || testoapp="\$TEST_OAPP_SOURCE_ADDRESS"

    local gas_limit="${GAS:-200000}"

    if $DRY_RUN; then
        echo "# Build executor options"
        echo "OPTIONS=\$(cast call $testoapp \"buildOptions(uint128)(bytes)\" $gas_limit --rpc-url $SOURCE_RPC)"
        echo ""
        echo "# Quote messaging fee"
        echo "QUOTE=\$(cast call $testoapp \"quote(uint32,string,bytes,bool)((uint256,uint256))\" $DEST_EID \"$MESSAGE\" \"\$OPTIONS\" false --rpc-url $SOURCE_RPC)"
        echo ""
        echo "# Send the message"
        echo "cast send $testoapp \"send(uint32,string,bytes)\" $DEST_EID \"$MESSAGE\" \"\$OPTIONS\" --value <fee> --rpc-url $SOURCE_RPC --private-key $PRIVATE_KEY --json"
        return 0
    fi

    if [[ "$testoapp" == "\$TEST_OAPP_SOURCE_ADDRESS" ]]; then
        die "no TestOApp address. Run 'make start' first"
    fi

    if ! $JSON_OUTPUT; then
        echo "Provider: layerzero"
        echo "Sending message: \"$MESSAGE\""
        echo "To EID: $DEST_EID"
        echo ""
    fi

    local options
    options=$(cast call "$testoapp" "buildOptions(uint128)(bytes)" "$gas_limit" --rpc-url "$SOURCE_RPC")

    local quote_result fee
    quote_result=$(cast call "$testoapp" "quote(uint32,string,bytes,bool)((uint256,uint256))" "$DEST_EID" "$MESSAGE" "$options" false --rpc-url "$SOURCE_RPC")
    fee=$(echo "$quote_result" | tr -d '()' | cut -d',' -f1 | tr -d ' ' | cut -d'[' -f1)

    if ! [[ "$fee" =~ ^[0-9]+$ ]] || [[ "$fee" == "0" ]]; then
        fee="1000000000000000"
    fi

    local tx_json tx_hash block_hex block
    tx_json=$(cast send "$testoapp" \
        "send(uint32,string,bytes)" \
        "$DEST_EID" \
        "$MESSAGE" \
        "$options" \
        --value "$fee" \
        --rpc-url "$SOURCE_RPC" \
        --private-key "$PRIVATE_KEY" \
        --json)

    tx_hash=$(echo "$tx_json" | jq -r '.transactionHash')
    block_hex=$(echo "$tx_json" | jq -r '.blockNumber')
    block=$((block_hex))

    save_to_cache "$tx_hash" "$block" "" "$MESSAGE" "$DEST_EID"

    if $JSON_OUTPUT; then
        echo "{\"provider\":\"layerzero\",\"tx_hash\":\"$tx_hash\",\"block\":$block}"
    else
        echo "TX: $tx_hash"
        echo "Block: $block"
        echo ""
        echo "Track with: make watch"
    fi
}

cmd_watch_layerzero() {
    load_addresses || true

    local destination_target
    destination_target=$(get_layerzero_dest_target_address 2>/dev/null) || destination_target="\$DVN_DEST_ADDRESS"

    if $DRY_RUN; then
        echo "# Poll operators for status and destination target logs"
        echo "curl -s http://localhost:3001/debug/v1/messages?limit=50"
        echo "cast logs --from-block <start> --address $destination_target --rpc-url $DEST_RPC"
        return 0
    fi

    load_watch_target_from_cache

    if ! $JSON_OUTPUT; then
        echo "═══════════════════════════════════════════════════════════════════"
        echo "Watching LayerZero message (timeout: ${TIMEOUT}s)"
        echo "═══════════════════════════════════════════════════════════════════"
        echo ""
    fi

    local start_time last_status target_verified target_tx_hash start_block dest_head cached_block
    start_time=$(date +%s)
    last_status=""
    target_verified=false
    target_tx_hash=""
    dest_head=$(cast block-number --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")
    start_block="$dest_head"

    if [[ -f "$CACHE_FILE" ]]; then
        cached_block=$(jq -r '.block // empty' "$CACHE_FILE" 2>/dev/null || true)
        if [[ "$cached_block" =~ ^[0-9]+$ ]]; then
            if (( cached_block <= dest_head )); then
                start_block="$cached_block"
            elif (( dest_head > 200 )); then
                start_block=$((dest_head - 200))
            else
                start_block=0
            fi
        fi
    fi

    while true; do
        local elapsed=$(( $(date +%s) - start_time ))
        if [[ $elapsed -ge $TIMEOUT ]]; then
            $JSON_OUTPUT && echo "{\"status\":\"timeout\",\"elapsed\":$elapsed}" || echo "Timeout after ${TIMEOUT}s"
            exit 1
        fi

        local best_status="" best_submission="" tx_hash_dest="" found_guid=""

        for i in 1 2 3; do
            local port=$((3000 + i))
            local response
            response=$(query_operator "$port" "$GUID" "$TX_HASH")

            if [[ "$response" != "{}" && -n "$response" && "$response" != "null" ]]; then
                if [[ -z "$GUID" || "$GUID" == "null" ]]; then
                    found_guid=$(echo "$response" | jq -r '.metadata.message_id // empty' 2>/dev/null || true)
                    [[ -n "$found_guid" && "$found_guid" != "null" ]] && GUID="$found_guid"
                fi

                local status submission_state submission_tx
                status=$(echo "$response" | jq -r '.status // "?"' 2>/dev/null || echo "?")
                submission_state=$(echo "$response" | jq -r '.submission.state // "Pending"' 2>/dev/null || echo "Pending")
                submission_tx=$(echo "$response" | jq -r '.submission.tx_hash // empty' 2>/dev/null || true)

                case $status in
                    Signed)
                        best_status="Signed"
                        best_submission="$submission_state"
                        tx_hash_dest="$submission_tx"
                        ;;
                    Processing) [[ "$best_status" != "Signed" ]] && best_status="Processing" ;;
                    Pending) [[ -z "$best_status" ]] && best_status="Pending" ;;
                    *) [[ -z "$best_status" ]] && best_status="$status" ;;
                esac
            fi
        done

        if [[ "$target_verified" == "false" && -n "$destination_target" ]]; then
            if check_layerzero_target_verified "$destination_target" "$start_block"; then
                target_verified=true
                target_tx_hash=$(get_layerzero_target_tx_hash "$destination_target" "$start_block")
            fi
        fi

        local current_status="${best_status}:${best_submission}:${target_verified}"
        if [[ "$current_status" != "$last_status" ]] && ! $JSON_OUTPUT; then
            local timestamp prev_status prev_submission prev_target
            timestamp=$(date +%H:%M:%S)
            IFS=':' read -r prev_status prev_submission prev_target <<< "$last_status"

            if [[ -n "$GUID" && "$last_status" == "" ]]; then
                echo "[$timestamp] Message ID: $GUID"
            fi
            [[ "$best_status" != "$prev_status" && -n "$best_status" ]] && echo "[$timestamp] $(format_status "$best_status")"
            if [[ "$best_submission" != "$prev_submission" && -n "$best_submission" && "$best_submission" != "Pending" ]]; then
                echo "[$timestamp] $(format_relayer_status "$best_submission" "$tx_hash_dest")"
            fi
            if [[ "$target_verified" == "true" && "$prev_target" != "true" ]]; then
                local target_msg="Destination target: verified on-chain"
                [[ -n "$target_tx_hash" ]] && target_msg="$target_msg (tx: $target_tx_hash)"
                echo "[$timestamp] $target_msg"
            fi

            last_status="$current_status"
        fi

        if [[ "$target_verified" == "true" ]]; then
            if $JSON_OUTPUT; then
                echo "{\"status\":\"verified\",\"message_id\":\"$GUID\",\"dest_tx\":\"$target_tx_hash\",\"elapsed\":$elapsed}"
            else
                echo ""
                echo "Message verified on destination chain"
                [[ -n "$GUID" ]] && echo "Message ID: $GUID"
                [[ -n "$target_tx_hash" && "$target_tx_hash" != "null" ]] && echo "Dest TX: $target_tx_hash"
            fi
            exit 0
        fi

        sleep 2
    done
}
