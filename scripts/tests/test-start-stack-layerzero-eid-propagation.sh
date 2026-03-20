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

require_line 'env_eid source' "expected start-stack to read source EID from env config"
require_line 'env_eid destination' "expected start-stack to read destination EID from env config"
require_line 'LZ_SOURCE_EID="$source_eid"' "expected stack deploy to receive source_eid"
require_line 'LZ_DEST_EID="$dest_eid"' "expected stack deploy to receive dest_eid"
require_line 'script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack' "expected start-stack to use the LayerZero stack script"
require_line '--sig "$stack_sig"' "expected start-stack to dispatch through the shared stack signature"
require_line '--multi' "expected stack deploy to use the multi-chain forge mode"

echo "start-stack layerzero eid propagation test passed"
