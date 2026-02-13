#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CCV_MSG_SCRIPT="$REPO_ROOT/scripts/providers/chainlink_ccv/msg.sh"

require_line() {
    local pattern="$1"
    local message="$2"
    grep -F -- "$pattern" "$CCV_MSG_SCRIPT" >/dev/null || {
        echo "$message" >&2
        exit 1
    }
}

require_line "ccv_refresh_epoch_if_needed()" "expected epoch refresh helper definition in chainlink_ccv msg provider script"
require_line "ccv_refresh_epoch_if_needed" "expected epoch refresh helper usage in chainlink_ccv msg provider script"

call_line="$(grep -n "ccv_refresh_epoch_if_needed" "$CCV_MSG_SCRIPT" | tail -n 1 | cut -d: -f1)"
send_line="$(grep -n 'tx_json="$(cast send "$onramp"' "$CCV_MSG_SCRIPT" | head -n 1 | cut -d: -f1)"

if [[ -z "$call_line" || -z "$send_line" ]]; then
    echo "failed to locate epoch refresh call or send transaction line" >&2
    exit 1
fi

if (( call_line >= send_line )); then
    echo "expected epoch refresh call to execute before cast send" >&2
    exit 1
fi

echo "chainlink ccv msg epoch refresh test passed"
