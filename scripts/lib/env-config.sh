#!/usr/bin/env bash
# env-config.sh — Shell helper library for reading environment JSON config.
#
# Expects ENV_CONFIG to be set to the path of the environment JSON file.
# Example: ENV_CONFIG=config/environments/local.json
#
# Usage: source scripts/lib/env-config.sh

set -euo pipefail

# Resolve the environment config file path.
# Uses ENV_CONFIG if set, otherwise derives from ENV (default: local).
env_config_file() {
    if [[ -n "${ENV_CONFIG:-}" ]]; then
        echo "$ENV_CONFIG"
    else
        echo "config/environments/${ENV:-local}.json"
    fi
}

# Read an arbitrary value from the environment JSON using a jq filter.
env_get() {
    local filter="$1"
    jq -r "$filter" "$(env_config_file)"
}

# Get chain ID for a role (source or destination).
env_chain_id() {
    local role="$1"
    env_get ".chains.${role}.chainId"
}

# Get EID for a role (source or destination).
env_eid() {
    local role="$1"
    env_get ".chains.${role}.eid"
}

# Get a predeploy address.
# Usage: env_predeploy <role> <namespace> <key>
# Example: env_predeploy destination symbioticCore vaultFactory
env_predeploy() {
    local role="$1" namespace="$2" key="$3"
    env_get ".chains.${role}.predeploys.${namespace}.${key}"
}

# Check if this is a local environment (anvil chain ID 31337).
env_is_local() {
    [[ "$(env_chain_id source)" == "31337" ]]
}

# Get the active provider name.
env_active_provider() {
    env_get '.activeProvider'
}

# Write a predeploy address into the environment JSON (used for local mock deploys).
# Usage: env_set_predeploy <role> <namespace> <key> <address>
env_set_predeploy() {
    local role="$1" namespace="$2" key="$3" address="$4"
    local config_file
    config_file="$(env_config_file)"
    local tmp="${config_file}.tmp"
    jq --arg addr "$address" ".chains.${role}.predeploys.${namespace}.${key} = \$addr" "$config_file" > "$tmp"
    mv "$tmp" "$config_file"
}

# Write a nested predeploy object.
# Usage: env_set_predeploy_object <role> <namespace> <json_object>
env_set_predeploy_object() {
    local role="$1" namespace="$2" json_obj="$3"
    local config_file
    config_file="$(env_config_file)"
    local tmp="${config_file}.tmp"
    jq --argjson obj "$json_obj" ".chains.${role}.predeploys.${namespace} = \$obj" "$config_file" > "$tmp"
    mv "$tmp" "$config_file"
}

# Get a relay timing parameter.
# Usage: env_relay <key>
# Example: env_relay epochDurationSeconds
env_relay() {
    local key="$1"
    env_get ".relay.${key}"
}

# Source deployment helpers after env readers so they can reuse env_chain_id.
# shellcheck disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/deployments.sh"

# Compatibility wrappers for existing scripts. These now read/write the
# dedicated deployments/<env>.json file rather than mutating env JSON.
env_deployment() {
    local role="$1" key="$2"
    deployment_get "$role" "$key"
}

env_has_deployments() {
    local role="$1"
    deployment_role_has_entries "$role"
}

env_set_deployment() {
    local role="$1" key="$2" address="$3"
    deployment_set "$role" "$key" "$address"
}

env_set_deployment_object() {
    local role="$1" key="$2" json_obj="$3"
    deployment_set_object "$role" "$key" "$json_obj"
}

env_clear_deployments() {
    local file
    ensure_deployments_file
    file="$(deployments_file)"
    cat > "$file" <<'EOF'
{
  "source": {},
  "destination": {}
}
EOF
}

env_generate_compose_env() {
    deployment_generate_sidecar_env "${1:-${GENERATED_DIR:-${PROJECT_ROOT:-$(cd "$(dirname "$(env_config_file)")/../.." && pwd)}/generated/${ENV:-local}}}"
}
