#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PREFLIGHT_SCRIPT="$REPO_ROOT/scripts/preflight-start.sh"

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

mkdir -p "$TMP_ROOT/config/environments"
mkdir -p "$TMP_ROOT/deployments"
mkdir -p "$TMP_ROOT/generated/local/oz-monitor/monitors"

# Create env JSON with immutable chainlink_ccv environment metadata
cat > "$TMP_ROOT/config/environments/local.json" <<'EOF'
{
  "version": 1,
  "name": "local",
  "activeProvider": "chainlink_ccv",
  "chains": {
    "source": {
      "name": "anvil",
      "chainId": 31337,
      "eid": 31337,
      "confirmations": 1,
      "blockTimeMs": 1000,
      "predeploys": {}
    },
    "destination": {
      "name": "anvil-settlement",
      "chainId": 31338,
      "eid": 31338,
      "confirmations": 1,
      "blockTimeMs": 1000,
      "predeploys": {}
    }
  },
  "relay": { "epochDurationSeconds": 60, "slashingWindowSeconds": 60, "epochStartDelaySeconds": 120 },
  "operator": { "logLevel": "debug", "eventPollInterval": "30s", "signJobInterval": "2s", "signWorkerCount": 2, "minBatchSize": 1 },
  "ozMonitor": { "cronSchedule": "*/5 * * * * *", "maxPastBlocks": 50 },
  "ozRelayer": { "defaultSpeed": "fast", "minBalanceWei": "10000000000000000" }
}
EOF

cat > "$TMP_ROOT/deployments/local.json" <<'EOF'
{
  "source": {
    "chainlinkCcv": {
      "ccv": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "onRamp": "0x1111111111111111111111111111111111111111",
      "offRamp": "0x3333333333333333333333333333333333333333"
    }
  },
  "destination": {
    "chainlinkCcv": {
      "ccv": "0xcccccccccccccccccccccccccccccccccccccccc",
      "onRamp": "0x4444444444444444444444444444444444444444",
      "offRamp": "0x2222222222222222222222222222222222222222"
    }
  }
}
EOF

cat > "$TMP_ROOT/generated/local/oz-monitor/monitors/ccip_message_sent.json" <<'EOF'
{
  "name": "ccv-monitor"
}
EOF

# Test 1: Should pass with full CCV deployments
PROJECT_ROOT="$TMP_ROOT" ENV=local ENV_CONFIG="$TMP_ROOT/config/environments/local.json" "$PREFLIGHT_SCRIPT" >/dev/null

# Test 2: Remove source CCV onRamp from deployments — should fail for missing onRamp
jq '.source.chainlinkCcv = {ccv: .source.chainlinkCcv.ccv, offRamp: .source.chainlinkCcv.offRamp}' \
    "$TMP_ROOT/deployments/local.json" > "$TMP_ROOT/deployments/local.json.tmp"
mv "$TMP_ROOT/deployments/local.json.tmp" "$TMP_ROOT/deployments/local.json"

if PROJECT_ROOT="$TMP_ROOT" ENV=local ENV_CONFIG="$TMP_ROOT/config/environments/local.json" "$PREFLIGHT_SCRIPT" >/dev/null 2>&1; then
    echo "expected preflight failure for missing source onRamp, but command succeeded" >&2
    exit 1
fi

# Test 3: Override via env var should recover
PROJECT_ROOT="$TMP_ROOT" \
ENV=local \
ENV_CONFIG="$TMP_ROOT/config/environments/local.json" \
CCV_SOURCE_ONRAMP_ADDRESS="0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
"$PREFLIGHT_SCRIPT" >/dev/null

echo "preflight-start test passed"
