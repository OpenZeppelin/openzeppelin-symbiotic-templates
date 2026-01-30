#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="$1"
LOG_PATH="$2"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OP_DIR="$ROOT_DIR/operator"

WEBHOOK_SECRET="${WEBHOOK_SECRET:-$(python3 - <<'PY'
print('a]' * 32)
PY
)}"
OZ_RELAYER_WEBHOOK_SECRET="${OZ_RELAYER_WEBHOOK_SECRET:-$(python3 - <<'PY'
print('b]' * 32)
PY
)}"
OZ_RELAYER_API_KEY="${OZ_RELAYER_API_KEY:-test-key}"

cd "$OP_DIR"
WEBHOOK_SECRET="$WEBHOOK_SECRET" \
OZ_RELAYER_WEBHOOK_SECRET="$OZ_RELAYER_WEBHOOK_SECRET" \
OZ_RELAYER_API_KEY="$OZ_RELAYER_API_KEY" \
nohup cargo run --quiet --bin operator -- -c "$CONFIG_PATH" > "$LOG_PATH" 2>&1 &

echo $!
