#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
SKIP_DOCKER_RESET="${SKIP_DOCKER_RESET:-0}"

SIDE_CAR_DIRS=(
    "$PROJECT_ROOT/data/sidecar-1"
    "$PROJECT_ROOT/data/sidecar-2"
    "$PROJECT_ROOT/data/sidecar-3"
)
MONITOR_CURSOR_FILE="$PROJECT_ROOT/data/oz-monitor/local_anvil_last_block.txt"

clean_dir_contents() {
    local dir="$1"
    mkdir -p "$dir"
    find "$dir" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
}

reset_docker_runtime() {
    if [[ "$SKIP_DOCKER_RESET" == "1" ]]; then
        return 0
    fi

    if ! command -v docker >/dev/null 2>&1; then
        echo "ERROR: docker is required for runtime reset" >&2
        exit 1
    fi

    echo "Resetting docker runtime services and ephemeral volumes..."
    (
        cd "$PROJECT_ROOT"
        # shellcheck disable=SC2086
        docker compose ${COMPOSE_FILES:-} --profile infra --profile dev down -v --remove-orphans >/dev/null 2>&1 || true
    )
}

reset_local_runtime_files() {
    echo "Clearing sidecar runtime state..."
    local dir
    for dir in "${SIDE_CAR_DIRS[@]}"; do
        clean_dir_contents "$dir"
    done

    echo "Clearing monitor cursor..."
    rm -f "$MONITOR_CURSOR_FILE"
}

main() {
    reset_docker_runtime
    reset_local_runtime_files
    echo "✓ Runtime state reset complete"
}

main "$@"
