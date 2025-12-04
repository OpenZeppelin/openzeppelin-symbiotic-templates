#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

docker compose --project-directory "$ROOT_DIR" down -v
rm -rf "$ROOT_DIR/deploy-data" "$ROOT_DIR/storage"

echo 'Devnet cleaned up.'
