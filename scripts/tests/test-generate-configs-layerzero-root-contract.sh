#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GEN_SCRIPT="$REPO_ROOT/scripts/generate-configs.sh"
TEMPLATES_DIR="$REPO_ROOT/config/templates"

TMP_ROOT="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

mkdir -p "$TMP_ROOT/config" "$TMP_ROOT/data/deploy-data"

cat > "$TMP_ROOT/config/root.config.json" <<'JSON'
{
  "active_provider": "layerzero",
  "providers": {
    "layerzero": {
      "source_chain_id": 11155111,
      "destination_chain_id": 84532,
      "source_eid": 40161,
      "destination_eid": 40245
    }
  }
}
JSON

cat > "$TMP_ROOT/data/deploy-data/deploy-state.json" <<'JSON'
{
  "version": 1,
  "providers": {
    "layerzero": {
      "source_chain_id": 11155111,
      "destination_chain_id": 84532,
      "source_eid": 40161,
      "destination_eid": 40245,
      "source": {
        "dvn": "0x1111111111111111111111111111111111111111",
        "send_uln": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "endpoint": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "executor": "0xcccccccccccccccccccccccccccccccccccccccc",
        "test_oapp": "0xdddddddddddddddddddddddddddddddddddddddd"
      },
      "destination": {
        "dvn": "0x2222222222222222222222222222222222222222",
        "receive_uln": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "endpoint": "0xffffffffffffffffffffffffffffffffffffffff",
        "test_oapp": "0x9999999999999999999999999999999999999999",
        "settlement": "0x1212121212121212121212121212121212121212"
      }
    }
  }
}
JSON

PROJECT_ROOT="$TMP_ROOT" \
ROOT_CONFIG_FILE="$TMP_ROOT/config/root.config.json" \
DEPLOY_DATA_DIR="$TMP_ROOT/data/deploy-data" \
OUTPUT_DIR="$TMP_ROOT/data/generated-config" \
TEMPLATES_DIR="$TEMPLATES_DIR" \
"$GEN_SCRIPT" >/dev/null

inode_of() {
    stat -f '%i' "$1"
}

operator_dir_inode_before="$(inode_of "$TMP_ROOT/data/generated-config/operator-1")"
monitor_dir_inode_before="$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor")"
monitor_monitors_inode_before="$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/monitors")"
monitor_networks_inode_before="$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/networks")"
monitor_triggers_inode_before="$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/triggers")"

PROJECT_ROOT="$TMP_ROOT" \
ROOT_CONFIG_FILE="$TMP_ROOT/config/root.config.json" \
DEPLOY_DATA_DIR="$TMP_ROOT/data/deploy-data" \
OUTPUT_DIR="$TMP_ROOT/data/generated-config" \
TEMPLATES_DIR="$TEMPLATES_DIR" \
"$GEN_SCRIPT" >/dev/null

[[ "$(inode_of "$TMP_ROOT/data/generated-config/operator-1")" == "$operator_dir_inode_before" ]] || {
    echo "expected operator output directory inode to be stable across regenerate" >&2
    exit 1
}
[[ "$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor")" == "$monitor_dir_inode_before" ]] || {
    echo "expected oz-monitor output directory inode to be stable across regenerate" >&2
    exit 1
}
[[ "$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/monitors")" == "$monitor_monitors_inode_before" ]] || {
    echo "expected oz-monitor/monitors inode to be stable across regenerate" >&2
    exit 1
}
[[ "$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/networks")" == "$monitor_networks_inode_before" ]] || {
    echo "expected oz-monitor/networks inode to be stable across regenerate" >&2
    exit 1
}
[[ "$(inode_of "$TMP_ROOT/data/generated-config/oz-monitor/triggers")" == "$monitor_triggers_inode_before" ]] || {
    echo "expected oz-monitor/triggers inode to be stable across regenerate" >&2
    exit 1
}

GEN_OPERATOR_CONFIG="$TMP_ROOT/data/generated-config/operator-1/config.json"

jq -e '.destination_chains == [84532]' "$GEN_OPERATOR_CONFIG" >/dev/null || {
    echo "expected destination_chains to use root destination_chain_id" >&2
    exit 1
}

jq -e '.oz_relayer.chain_relayers[0].chain_id == 84532' "$GEN_OPERATOR_CONFIG" >/dev/null || {
    echo "expected oz_relayer.chain_relayers[0].chain_id to use root destination_chain_id" >&2
    exit 1
}

jq -e '.layerzero.eid_to_chain_id["40161"] == 11155111' "$GEN_OPERATOR_CONFIG" >/dev/null || {
    echo "expected source EID->chain mapping from root config" >&2
    exit 1
}

jq -e '.layerzero.eid_to_chain_id["40245"] == 84532' "$GEN_OPERATOR_CONFIG" >/dev/null || {
    echo "expected destination EID->chain mapping from root config" >&2
    exit 1
}

jq -e '.layerzero.target_addresses["84532"] == "0x2222222222222222222222222222222222222222"' "$GEN_OPERATOR_CONFIG" >/dev/null || {
    echo "expected target_addresses key to use destination chain id" >&2
    exit 1
}

cat > "$TMP_ROOT/data/deploy-data/deploy-state.json" <<'JSON'
{
  "version": 1,
  "providers": {
    "layerzero": {
      "source_chain_id": 1,
      "destination_chain_id": 84532,
      "source_eid": 40161,
      "destination_eid": 40245,
      "source": {
        "dvn": "0x1111111111111111111111111111111111111111",
        "send_uln": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "endpoint": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "executor": "0xcccccccccccccccccccccccccccccccccccccccc",
        "test_oapp": "0xdddddddddddddddddddddddddddddddddddddddd"
      },
      "destination": {
        "dvn": "0x2222222222222222222222222222222222222222",
        "receive_uln": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "endpoint": "0xffffffffffffffffffffffffffffffffffffffff",
        "test_oapp": "0x9999999999999999999999999999999999999999",
        "settlement": "0x1212121212121212121212121212121212121212"
      }
    }
  }
}
JSON

drift_log="$(mktemp)"
if PROJECT_ROOT="$TMP_ROOT" \
    ROOT_CONFIG_FILE="$TMP_ROOT/config/root.config.json" \
    DEPLOY_DATA_DIR="$TMP_ROOT/data/deploy-data" \
    OUTPUT_DIR="$TMP_ROOT/data/generated-config" \
    TEMPLATES_DIR="$TEMPLATES_DIR" \
    "$GEN_SCRIPT" >"$drift_log" 2>&1; then
    echo "expected generate-configs to fail when root/deploy-data LayerZero chain IDs drift" >&2
    rm -f "$drift_log"
    exit 1
fi

grep -F "providers.layerzero.source_chain_id" "$drift_log" >/dev/null || {
    echo "expected source_chain_id drift error" >&2
    rm -f "$drift_log"
    exit 1
}

rm -f "$drift_log"

echo "generate-configs layerzero root contract test passed"
