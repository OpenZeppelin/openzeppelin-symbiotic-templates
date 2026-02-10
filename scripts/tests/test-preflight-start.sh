#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PREFLIGHT_SCRIPT="$REPO_ROOT/scripts/preflight-start.sh"

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/config"
mkdir -p "$TMP_ROOT/data/generated-config/operator-1"
mkdir -p "$TMP_ROOT/data/generated-config/operator-2"
mkdir -p "$TMP_ROOT/data/generated-config/operator-3"
mkdir -p "$TMP_ROOT/data/generated-config/oz-monitor/monitors"
mkdir -p "$TMP_ROOT/data/deploy-data"

cat > "$TMP_ROOT/config/root.config.json" <<'EOF'
{
  "active_provider": "chainlink_ccv",
  "providers": {
    "chainlink_ccv": {
      "mode": "symbiotic_mock",
      "source_chain_selector": 31337,
      "destination_chain_selector": 31338,
      "source_onramp_address": "",
      "source_offramp_address": "",
      "destination_onramp_address": "",
      "destination_offramp_address": ""
    }
  }
}
EOF

for idx in 1 2 3; do
    cat > "$TMP_ROOT/data/generated-config/operator-${idx}/config.json" <<'EOF'
{
  "provider": "chainlink_ccv"
}
EOF
done

cat > "$TMP_ROOT/data/generated-config/oz-monitor/monitors/ccip_message_sent.json" <<'EOF'
{
  "name": "ccv-monitor"
}
EOF

cat > "$TMP_ROOT/data/deploy-data/ccv_source_contracts.json" <<'EOF'
{
  "onRamp": "0x1111111111111111111111111111111111111111",
  "offRamp": "0x3333333333333333333333333333333333333333"
}
EOF

cat > "$TMP_ROOT/data/deploy-data/ccv_dest_contracts.json" <<'EOF'
{
  "onRamp": "0x4444444444444444444444444444444444444444",
  "offRamp": "0x2222222222222222222222222222222222222222"
}
EOF

touch "$TMP_ROOT/data/deploy-data/ccv-complete.marker"
touch "$TMP_ROOT/data/deploy-data/relay-infra-complete.marker"

PROJECT_ROOT="$TMP_ROOT" ROOT_CONFIG_FILE="$TMP_ROOT/config/root.config.json" "$PREFLIGHT_SCRIPT" >/dev/null

cat > "$TMP_ROOT/data/generated-config/operator-2/config.json" <<'EOF'
{
  "provider": "layerzero"
}
EOF

if PROJECT_ROOT="$TMP_ROOT" ROOT_CONFIG_FILE="$TMP_ROOT/config/root.config.json" "$PREFLIGHT_SCRIPT" >/dev/null 2>&1; then
    echo "expected preflight failure for provider mismatch, but command succeeded" >&2
    exit 1
fi

echo "preflight-start test passed"
