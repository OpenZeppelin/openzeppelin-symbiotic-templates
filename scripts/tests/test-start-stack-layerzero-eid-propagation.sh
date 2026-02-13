#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
START_STACK="$REPO_ROOT/scripts/start-stack.sh"

require_line() {
    local pattern="$1"
    local message="$2"
    grep -F -- "$pattern" "$START_STACK" >/dev/null || {
        echo "$message" >&2
        exit 1
    }
}

require_line '.providers.layerzero.source_eid | numbers' "expected start-stack to read providers.layerzero.source_eid"
require_line '.providers.layerzero.destination_eid | numbers' "expected start-stack to read providers.layerzero.destination_eid"
require_line '--sig "deploySource(uint32)" "$source_eid"' "expected deploySource(uint32) to receive source_eid"
require_line '--sig "deployDest(uint32)" "$dest_eid"' "expected deployDest(uint32) to receive dest_eid"
require_line '--sig "deploySource(address,uint32)" "$send_uln" "$source_eid"' "expected DVN source deploy to receive source_eid"
require_line '--sig "deployDest(address,address,uint32)" "$receive_uln" "$settlement_addr" "$dest_eid"' "expected DVN dest deploy to receive dest_eid"
require_line '--sig "configureSource(address,uint32)" "$src_dvn" "$dest_eid"' "expected configureSource to receive dest_eid"
require_line '--sig "configureDest(address,uint32)" "$dst_dvn" "$source_eid"' "expected configureDest to receive source_eid"

echo "start-stack layerzero eid propagation test passed"
