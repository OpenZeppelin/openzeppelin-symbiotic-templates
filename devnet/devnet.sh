#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Symbiotic LayerZero DVN - Devnet Management Script
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$SCRIPT_DIR"

# Colors and symbols
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

DONE="${GREEN}✓${NC}"
PROGRESS="${YELLOW}●${NC}"
PENDING="${BLUE}○${NC}"
FAILED="${RED}✗${NC}"

# =============================================================================
# Helper Functions
# =============================================================================

check_file() {
    [ -f "deploy-data/$1" ]
}

get_container_status() {
    local name="$1"
    local status
    status=$(docker compose ps --format json 2>/dev/null | jq -r "select(.Name | contains(\"$name\")) | .State" 2>/dev/null || echo "")
    echo "${status:-stopped}"
}

is_container_running() {
    local status
    status=$(get_container_status "$1")
    [ "$status" = "running" ]
}

is_container_healthy() {
    local health
    health=$(docker compose ps --format json 2>/dev/null | jq -r "select(.Name | contains(\"$1\")) | .Health" 2>/dev/null || echo "")
    [ "$health" = "healthy" ]
}

count_running_sidecars() {
    local count=0
    for i in 1 2 3 4; do
        is_container_running "relay-sidecar-$i" && ((count++)) || true
    done
    echo "$count"
}

print_header() {
    echo ""
    echo -e "${BLUE}=== Symbiotic LayerZero DVN Devnet ===${NC}"
    echo ""
}

# =============================================================================
# Status Display
# =============================================================================

print_chains_status() {
    echo -e "${BLUE}Chains:${NC}"

    if is_container_healthy "anvil-source"; then
        echo -e "  [${DONE}] anvil-source (31337)"
    elif is_container_running "anvil-source"; then
        echo -e "  [${PROGRESS}] anvil-source (31337) starting..."
    else
        echo -e "  [${PENDING}] anvil-source (31337)"
    fi

    if is_container_healthy "anvil-dest"; then
        echo -e "  [${DONE}] anvil-dest (31338)"
    elif is_container_running "anvil-dest"; then
        echo -e "  [${PROGRESS}] anvil-dest (31338) starting..."
    else
        echo -e "  [${PENDING}] anvil-dest (31338)"
    fi
    echo ""
}

print_deployment_status() {
    echo -e "${BLUE}Deployment:${NC}"

    local phases=(
        "source_chain_contracts.json:Symbiotic Source Chain"
        "dest_chain_contracts.json:Symbiotic Destination Chain"
        "driver_contracts.json:Driver"
        "lz_source_contracts.json:LayerZero Source"
        "lz_dest_contracts.json:LayerZero Dest"
        "test_oapp_source.json:TestOApp Source"
        "test_oapp_dest.json:TestOApp Dest"
    )

    local prev_done=true
    local deployer_running
    deployer_running=$(is_container_running "deployer" && echo "true" || echo "false")

    for phase in "${phases[@]}"; do
        local file="${phase%%:*}"
        local name="${phase##*:}"

        if check_file "$file"; then
            echo -e "  [${DONE}] $name"
            prev_done=true
        elif [ "$prev_done" = "true" ] && [ "$deployer_running" = "true" ]; then
            echo -e "  [${PROGRESS}] $name..."
            prev_done=false
        else
            echo -e "  [${PENDING}] $name"
            prev_done=false
        fi
    done
    echo ""
}

print_infra_status() {
    echo -e "${BLUE}Infrastructure:${NC}"

    # Genesis generator
    if check_file "genesis-complete.marker"; then
        echo -e "  [${DONE}] genesis-generator"
    elif is_container_running "genesis-generator"; then
        echo -e "  [${PROGRESS}] genesis-generator..."
    elif check_file "deployment-complete.marker"; then
        echo -e "  [${PENDING}] genesis-generator"
    else
        echo -e "  [${PENDING}] genesis-generator (waiting for deployment)"
    fi

    # Relay sidecars
    local sidecar_count
    sidecar_count=$(count_running_sidecars)
    if [ "$sidecar_count" -eq 4 ]; then
        echo -e "  [${DONE}] relay-sidecars (4/4)"
    elif [ "$sidecar_count" -gt 0 ]; then
        echo -e "  [${PROGRESS}] relay-sidecars ($sidecar_count/4)..."
    else
        echo -e "  [${PENDING}] relay-sidecars (0/4)"
    fi

    # DVN monitor (OZ Monitor + DVN Worker)
    if is_container_running "dvn-monitor"; then
        echo -e "  [${DONE}] dvn-monitor (running)"
    else
        echo -e "  [${PENDING}] dvn-monitor"
    fi
    echo ""
}

is_all_ready() {
    check_file "genesis-complete.marker" && \
    [ "$(count_running_sidecars)" -eq 4 ] && \
    is_container_running "dvn-monitor"
}

print_ready_message() {
    echo -e "${GREEN}=== Devnet Ready ===${NC}"
    echo ""
    echo "Endpoints:"
    echo "  Source RPC:  http://localhost:8545"
    echo "  Dest RPC:    http://localhost:8546"
    echo "  Sidecar API: http://localhost:8081"
    echo ""

    if check_file "source_chain_contracts.json"; then
        local dvn
        dvn=$(jq -r '.dvn.addr // empty' deploy-data/source_chain_contracts.json 2>/dev/null || echo "")
        [ -n "$dvn" ] && echo "  Source DVN: $dvn"
    fi

    if check_file "test_oapp_source.json"; then
        local oapp
        oapp=$(jq -r '.oapp // empty' deploy-data/test_oapp_source.json 2>/dev/null || echo "")
        [ -n "$oapp" ] && echo "  TestOApp:   $oapp"
    fi
    echo ""
    echo "Commands:"
    echo "  ./devnet.sh logs     - View DVN monitor logs"
    echo "  ./devnet.sh status   - Show current status"
    echo "  ./devnet.sh down     - Stop devnet"
    echo ""
}

# =============================================================================
# Commands
# =============================================================================

cmd_up() {
    local fresh=false
    [[ "${1:-}" == "--fresh" ]] && fresh=true

    echo -e "${BLUE}Starting Symbiotic LayerZero DVN Devnet...${NC}"
    echo ""

    if [ "$fresh" = true ]; then
        echo -e "${YELLOW}Fresh start requested, cleaning state...${NC}"
        docker compose down -v 2>/dev/null || true
        rm -rf storage/anvil-* deploy-data/*
        mkdir -p storage/anvil-source storage/anvil-dest deploy-data
        export FORCE_REDEPLOY=true
        echo ""
    fi

    # Check if relay-config exists, run generate_network.sh if not
    if [ ! -d "relay-config" ]; then
        echo "Relay config not found. Running generate_network.sh..."
        ./generate_network.sh
        echo ""
    fi

    # Ensure storage directories exist
    mkdir -p storage/anvil-source storage/anvil-dest

    # Start services
    docker compose up -d

    echo ""
    echo -e "Monitoring startup progress... (Ctrl+C to run in background)"
    echo ""

    # Progress loop
    trap 'echo -e "\n${YELLOW}Running in background. Use ./devnet.sh status to check progress.${NC}"; exit 0' INT

    while true; do
        # Move cursor up and clear (for clean refresh)
        tput cuu 20 2>/dev/null || true
        tput ed 2>/dev/null || true

        print_header
        print_chains_status
        print_deployment_status
        print_infra_status

        if is_all_ready; then
            print_ready_message
            break
        fi

        echo -e "${YELLOW}Press Ctrl+C to run in background${NC}"
        sleep 2
    done
}

cmd_down() {
    echo -e "${BLUE}Stopping devnet...${NC}"
    docker compose down
    echo -e "${GREEN}Devnet stopped. State preserved in storage/.${NC}"
    echo "Use './devnet.sh clean' to wipe all state."
}

cmd_clean() {
    echo -e "${BLUE}Stopping and cleaning all state...${NC}"
    docker compose down -v 2>/dev/null || true
    rm -rf deploy-data storage
    mkdir -p deploy-data storage/anvil-source storage/anvil-dest
    echo -e "${GREEN}Devnet stopped and all state wiped.${NC}"
}

cmd_status() {
    print_header

    echo -e "${BLUE}Containers:${NC}"
    docker compose ps --format "table {{.Name}}\t{{.Status}}" 2>/dev/null | tail -n +2 | while read line; do
        echo "  $line"
    done
    echo ""

    print_chains_status
    print_deployment_status
    print_infra_status

    if is_all_ready; then
        print_ready_message
    fi
}

cmd_logs() {
    local service="${2:-dvn-monitor}"
    echo -e "${BLUE}Following logs for: $service${NC}"
    echo "Press Ctrl+C to exit"
    echo ""
    docker compose logs -f "$service"
}

cmd_test() {
    echo -e "${BLUE}Running E2E Test...${NC}"
    echo ""

    # Verify devnet ready
    if ! is_all_ready; then
        echo -e "${RED}Devnet not ready. Run './devnet.sh up' first.${NC}"
        exit 1
    fi

    # Load addresses
    if [ ! -f "deploy-data/test_oapp_source.json" ] || [ ! -f "deploy-data/test_oapp_dest.json" ]; then
        echo -e "${RED}Deploy data not found. Run './devnet.sh up --fresh' first.${NC}"
        exit 1
    fi

    SOURCE_OAPP=$(jq -r '.oapp' deploy-data/test_oapp_source.json)
    DEST_OAPP=$(jq -r '.oapp' deploy-data/test_oapp_dest.json)

    # Verify contracts exist
    if ! cast code "$SOURCE_OAPP" --rpc-url http://localhost:8545 2>/dev/null | grep -q "0x"; then
        echo -e "${RED}ERROR: Source TestOApp not deployed at $SOURCE_OAPP${NC}"
        echo "Run './devnet.sh up --fresh' to redeploy."
        exit 1
    fi

    # Get initial count
    BEFORE=$(cast call "$DEST_OAPP" "messagesReceived()(uint256)" --rpc-url http://localhost:8546 2>/dev/null || echo "0")
    echo "Messages received before: $BEFORE"

    # Send ping
    echo "Sending ping to destination chain..."
    cast send "$SOURCE_OAPP" "ping(uint32)" 31338 \
        --value 0.01ether \
        --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
        --rpc-url http://localhost:8545 \
        --quiet 2>/dev/null || {
            echo -e "${RED}Failed to send ping transaction${NC}"
            exit 1
        }

    echo "Ping sent. Waiting for DVN to process (up to 60s)..."

    # Wait for DVN processing
    for i in {1..12}; do
        sleep 5
        AFTER=$(cast call "$DEST_OAPP" "messagesReceived()(uint256)" --rpc-url http://localhost:8546 2>/dev/null || echo "0")
        if [ "$AFTER" -gt "$BEFORE" ]; then
            echo ""
            echo -e "${GREEN}✓ E2E Test PASSED${NC}"
            echo "  Message sent from source and received on destination"
            echo "  Messages received: $BEFORE → $AFTER"
            exit 0
        fi
        echo -n "."
    done

    echo ""
    echo -e "${RED}✗ E2E Test FAILED${NC}"
    echo "  Message was sent but not received within 60 seconds"
    echo "  Check DVN monitor logs: ./devnet.sh logs dvn-monitor"
    exit 1
}

cmd_reload() {
    local target="${1:-}"

    if [ -z "$target" ]; then
        echo "Usage: $0 reload <dvn|oapp> [--watch]"
        echo ""
        echo "Hot reload contracts without losing state."
        echo ""
        echo "Targets:"
        echo "  dvn   Reload SymbioticLayerZeroDVN (both chains)"
        echo "  oapp  Reload TestOApp (both chains)"
        echo ""
        echo "Options:"
        echo "  --watch  Auto-reload on file changes (requires watchexec)"
        exit 1
    fi

    # Handle --watch flag
    if [ "$target" = "--watch" ] || [ "${2:-}" = "--watch" ]; then
        if ! command -v watchexec &> /dev/null; then
            echo -e "${RED}watchexec not found. Install with: brew install watchexec${NC}"
            exit 1
        fi
        echo -e "${BLUE}Starting watch mode...${NC}"
        echo "Will reload DVN on any .sol file change in src/"
        echo "Press Ctrl+C to stop"
        watchexec -w "$PROJECT_ROOT/src" -e sol -c clear -- "$0" reload dvn
        return
    fi

    # Rebuild contracts
    echo -e "${BLUE}Building contracts...${NC}"
    (cd "$PROJECT_ROOT" && forge build) || {
        echo -e "${RED}Build failed${NC}"
        exit 1
    }

    case "$target" in
        dvn)
            reload_contract "SymbioticLayerZeroDVN" \
                "$(jq -r '.dvn.addr' deploy-data/source_chain_contracts.json)" \
                "http://localhost:8545" "source"
            reload_contract "SymbioticLayerZeroDVN" \
                "$(jq -r '.dvn.addr' deploy-data/dest_chain_contracts.json)" \
                "http://localhost:8546" "dest"
            ;;
        oapp)
            reload_contract "TestOApp" \
                "$(jq -r '.oapp' deploy-data/test_oapp_source.json)" \
                "http://localhost:8545" "source"
            reload_contract "TestOApp" \
                "$(jq -r '.oapp' deploy-data/test_oapp_dest.json)" \
                "http://localhost:8546" "dest"
            ;;
        *)
            echo -e "${RED}Unknown target: $target${NC}"
            echo "Use: dvn or oapp"
            exit 1
            ;;
    esac

    echo ""
    echo -e "${GREEN}Hot reload complete!${NC}"
}

reload_contract() {
    local contract_name="$1"
    local address="$2"
    local rpc_url="$3"
    local chain_name="$4"

    echo "Reloading $contract_name at $address ($chain_name)..."

    # Get deployed bytecode from compiled artifacts
    local bytecode
    bytecode=$(cd "$PROJECT_ROOT" && forge inspect "$contract_name" deployedBytecode 2>/dev/null)

    if [ -z "$bytecode" ] || [ "$bytecode" = "0x" ]; then
        echo -e "${RED}ERROR: Could not get bytecode for $contract_name${NC}"
        return 1
    fi

    # Inject bytecode at address using anvil_setCode
    cast rpc anvil_setCode "$address" "$bytecode" --rpc-url "$rpc_url" > /dev/null 2>&1

    if [ $? -eq 0 ]; then
        echo -e "  ${GREEN}✓${NC} Reloaded $contract_name at $address"
    else
        echo -e "  ${RED}✗${NC} Failed to reload $contract_name"
        return 1
    fi
}

cmd_restart() {
    cmd_clean
    echo ""
    cmd_up
}

usage() {
    echo "Symbiotic LayerZero DVN - Devnet Management"
    echo ""
    echo "Usage: $0 <command>"
    echo ""
    echo "Commands:"
    echo "  up [--fresh]     Start devnet (--fresh wipes state first)"
    echo "  down             Stop containers (state preserved)"
    echo "  clean            Stop and wipe all state"
    echo "  status           Show current status"
    echo "  logs [service]   Tail logs (default: dvn-monitor)"
    echo "  test             Run E2E test (send msg, verify delivery)"
    echo "  reload <target>  Hot reload contracts (dvn|oapp)"
    echo "  restart          Clean restart (clean + up)"
    echo ""
    echo "Examples:"
    echo "  $0 up                # Start with existing state"
    echo "  $0 up --fresh        # Fresh start, wipe everything"
    echo "  $0 test              # Send ping, verify delivery"
    echo "  $0 reload dvn        # Hot reload DVN contract"
    echo "  $0 reload --watch    # Auto-reload on file changes"
    echo "  $0 logs dvn-monitor  # View DVN monitor logs"
}

# =============================================================================
# Main
# =============================================================================

case "${1:-}" in
    up)      cmd_up "$2" ;;
    down)    cmd_down ;;
    clean)   cmd_clean ;;
    status)  cmd_status ;;
    logs)    cmd_logs "$@" ;;
    test)    cmd_test ;;
    reload)  cmd_reload "$2" "$3" ;;
    restart) cmd_restart ;;
    *)       usage ;;
esac
