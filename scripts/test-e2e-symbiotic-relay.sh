#!/bin/bash
# E2E Test wrapper - calls unified msg script
#
# This script is kept for backwards compatibility.
# Prefer using: make e2e  or  ./scripts/msg e2e
#
# Usage: ./scripts/test-e2e-symbiotic-relay.sh [timeout]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TIMEOUT="${1:-120}"

exec "$SCRIPT_DIR/msg" e2e --timeout "$TIMEOUT" --message "Hello from e2e test"
