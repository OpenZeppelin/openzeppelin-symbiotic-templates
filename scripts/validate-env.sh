#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/lib/common.sh"

MAX_EPOCH_VALIDITY_SECONDS="${MAX_EPOCH_VALIDITY_SECONDS:-7200}"
VALIDATE_MANAGED_OPERATORS="${VALIDATE_MANAGED_OPERATORS:-0}"

failures=()

record_failure() {
    failures+=("$1")
}

lower_hex() {
    printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]'
}

require_file() {
    local file="$1"
    [[ -f "$file" ]] || record_failure "missing file: $file"
}

require_address() {
    local value="$1"
    local label="$2"
    if [[ -z "$value" || "$value" == "null" ]]; then
        record_failure "missing ${label} in $(deployments_file)"
        return 1
    fi
    if [[ ! "$value" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        record_failure "invalid ${label}: ${value}"
        return 1
    fi
}

check_code() {
    local rpc_url="$1"
    local address="$2"
    local label="$3"
    local code

    require_address "$address" "$label" || return 0
    code="$(cast code "$address" --rpc-url "$rpc_url" 2>/dev/null || echo "0x")"
    if [[ -z "$code" || "$code" == "0x" ]]; then
        record_failure "${label} has no code at ${address}"
    fi
}

validate_layerzero() {
    local src_dvn dst_dvn src_oapp dst_oapp settlement actual_settlement

    src_dvn="$(env_deployment source dvn 2>/dev/null || true)"
    dst_dvn="$(env_deployment destination dvn 2>/dev/null || true)"
    src_oapp="$(env_deployment source testOApp 2>/dev/null || true)"
    dst_oapp="$(env_deployment destination testOApp 2>/dev/null || true)"
    settlement="$(env_deployment destination relayInfra.settlement 2>/dev/null || true)"

    check_code "$SOURCE_RPC" "$src_dvn" "source DVN"
    check_code "$DEST_RPC" "$dst_dvn" "destination DVN"
    check_code "$SOURCE_RPC" "$src_oapp" "source TestOApp"
    check_code "$DEST_RPC" "$dst_oapp" "destination TestOApp"
    check_code "$DEST_RPC" "$settlement" "relayInfra.settlement"

    if [[ -n "$dst_dvn" && -n "$settlement" ]]; then
        actual_settlement="$(cast call "$dst_dvn" "settlement()(address)" --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        actual_settlement="$(lower_hex "$actual_settlement")"
        if [[ -n "$actual_settlement" && "$actual_settlement" != "$(lower_hex "$settlement")" ]]; then
            record_failure "destination DVN settlement mismatch: expected ${settlement}, got ${actual_settlement}"
        fi
    fi
}

validate_chainlink_ccv() {
    local src_ccv dst_ccv src_onramp dst_offramp settlement actual_settlement

    src_ccv="$(env_deployment source chainlinkCcv.ccv 2>/dev/null || true)"
    dst_ccv="$(env_deployment destination chainlinkCcv.ccv 2>/dev/null || true)"
    src_onramp="$(env_deployment source chainlinkCcv.onRamp 2>/dev/null || true)"
    dst_offramp="$(env_deployment destination chainlinkCcv.offRamp 2>/dev/null || true)"
    settlement="$(env_deployment destination chainlinkCcv.settlement 2>/dev/null || true)"

    check_code "$SOURCE_RPC" "$src_ccv" "source CCV"
    check_code "$DEST_RPC" "$dst_ccv" "destination CCV"
    check_code "$SOURCE_RPC" "$src_onramp" "source onRamp"
    check_code "$DEST_RPC" "$dst_offramp" "destination offRamp"

    if [[ -n "$settlement" && "$settlement" != "null" ]]; then
        check_code "$DEST_RPC" "$settlement" "destination CCV settlement"
        actual_settlement="$(cast call "$dst_ccv" "settlement()(address)" --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        actual_settlement="$(lower_hex "$actual_settlement")"
        if [[ -n "$actual_settlement" && "$actual_settlement" != "$(lower_hex "$settlement")" ]]; then
            record_failure "destination CCV settlement mismatch: expected ${settlement}, got ${actual_settlement}"
        fi
    fi
}

validate_genesis() {
    local settlement epoch capture now age

    settlement="$(env_deployment destination relayInfra.settlement 2>/dev/null || true)"
    if [[ -z "$settlement" || "$settlement" == "null" ]]; then
        return 0
    fi

    epoch="$(cast call "$settlement" "getLastCommittedHeaderEpoch()(uint48)" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")"
    epoch="$(printf '%s' "$epoch" | tr -d '[:space:]')"
    if [[ ! "$epoch" =~ ^[0-9]+$ ]] || [[ "$epoch" -eq 0 ]]; then
        record_failure "genesis missing: no committed settlement epoch found"
        return 0
    fi

    capture="$(cast call "$settlement" "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)" "$epoch" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")"
    capture="$(printf '%s' "$capture" | tr -d '[:space:]')"
    if [[ ! "$capture" =~ ^[0-9]+$ ]] || [[ "$capture" -eq 0 ]]; then
        record_failure "genesis invalid: settlement epoch ${epoch} has no capture timestamp"
        return 0
    fi

    now="$(date +%s)"
    age=$((now - capture))
    if (( age >= MAX_EPOCH_VALIDITY_SECONDS )); then
        record_failure "genesis stale: age ${age}s > ${MAX_EPOCH_VALIDITY_SECONDS}s"
    fi
}

validate_managed_operator_keys() {
    local key_registry
    key_registry="$(env_deployment destination relayInfra.keyRegistry 2>/dev/null || true)"
    if [[ -z "$key_registry" || "$key_registry" == "null" ]]; then
        record_failure "missing relayInfra.keyRegistry in $(deployments_file)"
        return 0
    fi

    local i op_addr key15 key11 balance
    for i in 0 1 2; do
        if ! op_addr="$(get_operator_address "$i" 2>/dev/null)"; then
            record_failure "managed operator $((i + 1)) key missing"
            continue
        fi

        key15="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 15 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        key11="$(cast call "$key_registry" "getKey(address,uint8)(bytes)" "$op_addr" 11 --rpc-url "$DEST_RPC" 2>/dev/null || true)"
        [[ -n "$key15" && "$key15" != "0x" ]] || record_failure "operator $((i + 1)) missing BLS key tag 15"
        [[ -n "$key11" && "$key11" != "0x" ]] || record_failure "operator $((i + 1)) missing BLS key tag 11"

        balance="$(cast balance "$op_addr" --rpc-url "$DEST_RPC" 2>/dev/null || echo "0")"
        [[ "$balance" != "0" ]] || record_failure "operator $((i + 1)) has zero native balance on destination chain"
    done
}

main() {
    local active_provider

    require_file "$(env_config_file)"
    require_file "$(deployments_file)"

    if ! is_local; then
        [[ -n "${SOURCE_RPC:-}" ]] || record_failure "SOURCE RPC is not configured"
        [[ -n "${DEST_RPC:-}" ]] || record_failure "DEST RPC is not configured"
        [[ -n "${PRIVATE_KEY:-}" ]] || record_failure "PRIVATE_KEY is not configured"
    fi

    active_provider="$(get_active_provider)"

    case "$active_provider" in
        layerzero)
            validate_layerzero
            ;;
        chainlink_ccv)
            validate_chainlink_ccv
            ;;
        *)
            record_failure "unsupported provider: ${active_provider}"
            ;;
    esac

    validate_genesis

    if [[ "$VALIDATE_MANAGED_OPERATORS" == "1" ]]; then
        validate_managed_operator_keys
    fi

    if (( ${#failures[@]} > 0 )); then
        printf 'Validation failed:\n' >&2
        printf '  - %s\n' "${failures[@]}" >&2
        exit 1
    fi

    echo "Validation passed for provider: ${active_provider}"
}

main "$@"
