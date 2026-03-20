#!/usr/bin/env bash
# deployments.sh — Shell helper library for reading/writing deployment JSON.
#
# Expects ENV to select deployments/<env>.json unless DEPLOYMENTS_FILE is set.
#
# Usage: source scripts/lib/deployments.sh

set -euo pipefail

deployments_file() {
    local root="${PROJECT_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
    if [[ -n "${DEPLOYMENTS_FILE:-}" ]]; then
        echo "$DEPLOYMENTS_FILE"
    else
        echo "$root/deployments/${ENV:-local}.json"
    fi
}

ensure_deployments_file() {
    local file
    file="$(deployments_file)"
    mkdir -p "$(dirname "$file")"
    if [[ ! -f "$file" ]]; then
        cat > "$file" <<'EOF'
{
  "source": {},
  "destination": {}
}
EOF
    fi
}

deployments_get() {
    local filter="$1"
    local file
    file="$(deployments_file)"
    [[ -f "$file" ]] || return 1
    jq -r "$filter" "$file"
}

deployment_get() {
    local role="$1"
    local key="$2"
    deployments_get ".${role}.${key} // empty"
}

deployment_set() {
    local role="$1"
    local key="$2"
    local address="$3"
    local file tmp
    ensure_deployments_file
    file="$(deployments_file)"
    tmp="${file}.tmp"
    jq --arg addr "$address" ".${role}.${key} = \$addr" "$file" > "$tmp"
    mv "$tmp" "$file"
}

deployment_set_object() {
    local role="$1"
    local key="$2"
    local json_obj="$3"
    local file tmp
    ensure_deployments_file
    file="$(deployments_file)"
    tmp="${file}.tmp"
    jq --argjson obj "$json_obj" ".${role}.${key} = \$obj" "$file" > "$tmp"
    mv "$tmp" "$file"
}

deployment_role_has_entries() {
    local role="$1"
    local file count
    file="$(deployments_file)"
    [[ -f "$file" ]] || return 1
    count="$(jq -r ".${role} | length" "$file" 2>/dev/null || echo "0")"
    [[ "$count" -gt 0 ]]
}

deployment_generate_sidecar_env() {
    local output_dir="${1:-${GENERATED_DIR:-${PROJECT_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}/generated/${ENV:-local}}}"
    local out="${output_dir}/sidecar.env"
    local driver

    mkdir -p "$output_dir"
    driver="$(deployment_get destination relayInfra.driver 2>/dev/null || true)"
    if [[ -n "$driver" && "$driver" != "null" ]]; then
        driver="$(printf '%s' "$driver" | tr '[:upper:]' '[:lower:]')"
    else
        driver=""
    fi

    cat > "$out" <<EOF
# Generated from $(deployments_file) — do not edit
DRIVER_ADDRESS=${driver}
DRIVER_CHAIN_ID=$(env_chain_id destination)
SOURCE_CHAIN_ID=$(env_chain_id source)
EOF
}
