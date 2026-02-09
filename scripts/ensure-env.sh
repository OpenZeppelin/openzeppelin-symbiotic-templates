#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ENV_FILE="$PROJECT_ROOT/.env"
KEYSTORE_DIR="$PROJECT_ROOT/config/oz-relayer/keys"
REQUIRED_KEYSTORES=(
    "$KEYSTORE_DIR/signer-1.json"
    "$KEYSTORE_DIR/signer-2.json"
    "$KEYSTORE_DIR/signer-3.json"
)

missing=()

if [[ ! -f "$ENV_FILE" ]]; then
    missing+=(".env")
fi

for keystore in "${REQUIRED_KEYSTORES[@]}"; do
    if [[ ! -f "$keystore" ]]; then
        missing+=("$(basename "$keystore")")
    fi
done

if [[ ${#missing[@]} -eq 0 ]]; then
    echo "Environment already initialized (.env + OZ relayer keystores present)."
    exit 0
fi

echo "Environment bootstrap required (missing: ${missing[*]})."
echo "Generating local environment via scripts/setup.sh..."
"$PROJECT_ROOT/scripts/setup.sh"
