#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

ACTIVE_PROVIDER="${1:-}"
WAIT_ONLY="${2:-}"
MAX_ATTEMPTS="${MAX_ATTEMPTS:-3}"
RETRY_DELAY_SECONDS="${RETRY_DELAY_SECONDS:-5}"
HEALTH_TIMEOUT_SECONDS="${HEALTH_TIMEOUT_SECONDS:-120}"
MONITOR_SYNC_TIMEOUT_SECONDS="${MONITOR_SYNC_TIMEOUT_SECONDS:-180}"
MONITOR_SYNC_POLL_SECONDS="${MONITOR_SYNC_POLL_SECONDS:-2}"
MONITOR_MAX_LAG_BLOCKS="${MONITOR_MAX_LAG_BLOCKS:-20}"
MONITOR_SOURCE_RPC="${MONITOR_SOURCE_RPC:-http://localhost:8545}"
MONITOR_CURSOR_FILE="${MONITOR_CURSOR_FILE:-$PROJECT_ROOT/data/oz-monitor/local_anvil_last_block.txt}"
FORCE_RECREATE_RELAYER="${FORCE_RECREATE_RELAYER:-0}"

if [[ -z "$ACTIVE_PROVIDER" ]]; then
    echo "ERROR: usage: $0 <active_provider> [--wait-only]" >&2
    exit 1
fi

case "$ACTIVE_PROVIDER" in
    layerzero|chainlink_ccv)
        ;;
    *)
        echo "ERROR: unsupported active_provider '$ACTIVE_PROVIDER'" >&2
        exit 1
        ;;
esac

CRITICAL_CONTAINERS=(
    anvil
    anvil-settlement
    redis
    oz-monitor
    oz-relayer
    symbiotic-relay-1
    symbiotic-relay-2
    symbiotic-relay-3
    operator-1
    operator-2
    operator-3
)

ensure_docker_available() {
    if ! docker info >/dev/null 2>&1; then
        echo "ERROR: Docker daemon is not reachable. Start Docker and retry." >&2
        exit 1
    fi
}

service_state() {
    local container="$1"
    docker inspect --format '{{.State.Status}}' "$container" 2>/dev/null || echo "missing"
}

service_health() {
    local container="$1"
    docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$container" 2>/dev/null || echo "none"
}

print_diagnostics() {
    local reason="$1"
    shift
    local containers=("$@")

    echo "ERROR: $reason" >&2
    echo "" >&2
    echo "Container summary:" >&2
    docker compose --profile infra --profile dev ps || true
    echo "" >&2

    if [[ ${#containers[@]} -gt 0 ]]; then
        echo "Failing services diagnostics:" >&2
        for container in "${containers[@]}"; do
            local state health
            state="$(service_state "$container")"
            health="$(service_health "$container")"
            echo "- $container (state=$state, health=$health)" >&2
            echo "  Last 40 logs:" >&2
            docker logs --tail 40 "$container" 2>&1 | sed 's/^/    /' >&2 || true
            echo "" >&2
        done
    fi
}

start_compose() {
    local attempt=1
    local started=0
    local output

    while [[ $attempt -le $MAX_ATTEMPTS ]]; do
        if output="$(docker compose --profile infra --profile dev up -d --remove-orphans 2>&1)"; then
            started=1
            break
        fi

        echo "$output" >&2
        if [[ $attempt -lt $MAX_ATTEMPTS ]]; then
            echo "WARN: service startup attempt ${attempt}/${MAX_ATTEMPTS} failed, retrying in ${RETRY_DELAY_SECONDS}s..." >&2
            sleep "$RETRY_DELAY_SECONDS"
        fi
        attempt=$((attempt + 1))
    done

    if [[ $started -ne 1 ]]; then
        print_diagnostics "failed to start services for provider '$ACTIVE_PROVIDER'" "${CRITICAL_CONTAINERS[@]}"
        exit 1
    fi
}

wait_for_health() {
    local start_ts now elapsed
    start_ts="$(date +%s)"

    while true; do
        local pending=()
        local failing=()

        for container in "${CRITICAL_CONTAINERS[@]}"; do
            local state health
            state="$(service_state "$container")"
            health="$(service_health "$container")"

            case "$state" in
                running)
                    case "$health" in
                        healthy|none)
                            ;;
                        starting)
                            pending+=("$container")
                            ;;
                        unhealthy)
                            failing+=("$container")
                            ;;
                        *)
                            pending+=("$container")
                            ;;
                    esac
                    ;;
                created|restarting)
                    pending+=("$container")
                    ;;
                exited|dead|missing)
                    failing+=("$container")
                    ;;
                *)
                    pending+=("$container")
                    ;;
            esac
        done

        if [[ ${#failing[@]} -gt 0 ]]; then
            print_diagnostics "critical services unhealthy for provider '$ACTIVE_PROVIDER'" "${failing[@]}"
            exit 1
        fi

        if [[ ${#pending[@]} -eq 0 ]]; then
            echo "✓ Critical services are healthy for provider: $ACTIVE_PROVIDER"
            return 0
        fi

        now="$(date +%s)"
        elapsed=$((now - start_ts))
        if [[ $elapsed -ge $HEALTH_TIMEOUT_SECONDS ]]; then
            print_diagnostics "startup health timeout after ${HEALTH_TIMEOUT_SECONDS}s for provider '$ACTIVE_PROVIDER'" "${pending[@]}"
            exit 1
        fi

        sleep 2
    done
}

maybe_recreate_relayer() {
    if [[ "$FORCE_RECREATE_RELAYER" != "1" ]]; then
        return 0
    fi

    echo "Force-recreating oz-relayer to refresh Redis consumer registration..."
    docker compose --profile dev up -d --force-recreate oz-relayer >/dev/null
}

wait_for_monitor_sync() {
    local start_ts now elapsed
    local head cursor lag

    start_ts="$(date +%s)"

    while true; do
        head="$(cast block-number --rpc-url "$MONITOR_SOURCE_RPC" 2>/dev/null || true)"
        if [[ "$head" =~ ^[0-9]+$ ]] && [[ -f "$MONITOR_CURSOR_FILE" ]]; then
            cursor="$(tr -d '[:space:]' < "$MONITOR_CURSOR_FILE" 2>/dev/null || true)"
            if [[ "$cursor" =~ ^[0-9]+$ ]]; then
                if (( cursor > head )); then
                    echo "WARN: oz-monitor cursor ($cursor) is ahead of chain head ($head); resetting cursor to avoid stale position."
                    printf '%s\n' "$head" > "$MONITOR_CURSOR_FILE"
                    cursor="$head"
                fi

                lag=$((head - cursor))
                if (( lag <= MONITOR_MAX_LAG_BLOCKS )); then
                    echo "✓ oz-monitor synced (lag ${lag} blocks)"
                    return 0
                fi
            fi
        fi

        now="$(date +%s)"
        elapsed=$((now - start_ts))
        if (( elapsed >= MONITOR_SYNC_TIMEOUT_SECONDS )); then
            echo "WARN: oz-monitor did not sync within ${MONITOR_SYNC_TIMEOUT_SECONDS}s; continuing startup."
            if [[ -n "${lag:-}" ]]; then
                echo "WARN: current monitor lag is ${lag} blocks; early webhooks may be delayed."
            fi
            return 0
        fi

        sleep "$MONITOR_SYNC_POLL_SECONDS"
    done
}

if [[ "$WAIT_ONLY" != "--wait-only" ]]; then
    ensure_docker_available
    start_compose
    maybe_recreate_relayer
fi
ensure_docker_available
wait_for_health
wait_for_monitor_sync
