#!/bin/bash
# LayerZero DVN Worker Script
# Called by OZ Monitor when a JobAssigned event is detected
#
# Input: JSON via stdin (OZ Monitor format)
# Output: Exit code 0 on success, non-zero on failure
#
# Environment variables (must be set):
#   DEST_RPC_URL - Destination chain RPC endpoint
#   DEST_DVN_ADDRESS - DVN contract address on destination chain
#   SIDECAR_URL - Symbiotic relay sidecar URL
#   PRIVATE_KEY - Private key for signing transactions

set -euo pipefail

# Debug: show environment
echo "DVN Worker: Environment check" >&2
echo "  DEST_RPC_URL: ${DEST_RPC_URL:-NOT SET}" >&2
echo "  DEST_DVN_ADDRESS: ${DEST_DVN_ADDRESS:-NOT SET}" >&2
echo "  SIDECAR_URL: ${SIDECAR_URL:-NOT SET}" >&2

# Read JSON input from stdin
INPUT_JSON=$(cat)

# Log the incoming event (truncate to show we received it)
echo "DVN Worker: Received JobAssigned event (${#INPUT_JSON} bytes)" >&2

# Pass the JSON to the Rust binary via stdin
# The binary will parse it and process the event
echo "$INPUT_JSON" | /app/layerzero_dvn_worker

exit_code=$?

if [ $exit_code -eq 0 ]; then
    echo "DVN Worker: Successfully processed event" >&2
else
    echo "DVN Worker: Failed to process event (exit code: $exit_code)" >&2
fi

exit $exit_code
