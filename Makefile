.PHONY: help start stop clean install
.PHONY: restart-operators restart-monitor restart-relayer restart-relays
.PHONY: dev-operator rebuild-operators test test-contracts e2e
.PHONY: test-scripts
.PHONY: logs-operators logs-operator-1 logs-operator-2 logs-operator-3
.PHONY: logs-monitor logs-relayer logs-relays
.PHONY: status setup configure addresses shell ensure-env refresh-epoch reset-runtime
.PHONY: send watch msg-status
.PHONY: deploy-ccv-contracts configure-ccv-contracts

# Default private key for anvil (account 0)
PRIVATE_KEY ?= 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

ROOT_CONFIG_FILE := config/root.config.json
ROOT_CONFIG_FILE_ABS := $(abspath $(ROOT_CONFIG_FILE))

# ═══════════════════════════════════════════════════════════════════════════════
# HELP
# ═══════════════════════════════════════════════════════════════════════════════

help:
	@echo "Operator - Development Commands (3-Operator Architecture)"
	@echo "═══════════════════════════════════════════════════════════════════"
	@echo ""
	@echo "Primary Commands:"
	@echo "  make install            Install dependencies (contracts npm packages)"
	@echo "  make start              Smart start (provider-aware deploy + start)"
	@echo "  make stop               Stop all containers (preserve state)"
	@echo "  make clean              Full reset (stop + remove volumes + deploy state)"
	@echo ""
	@echo "Service Restarts:"
	@echo "  make restart-operators  Rebuild and restart all 3 operators"
	@echo "  make restart-monitor    Restart oz-monitor (config reload)"
	@echo "  make restart-relayer    Restart oz-relayer"
	@echo "  make restart-relays     Restart symbiotic-relay-1/2/3"
	@echo ""
	@echo "Development:"
	@echo "  make dev-operator       Run operator-1 locally (cargo run)"
	@echo "  make rebuild-operators  Docker rebuild + restart all operators"
	@echo "  make setup              (Optional) regenerate .env + local keys"
	@echo "  make shell              Interactive shell with addresses loaded"
	@echo ""
	@echo "Testing:"
	@echo "  make test               Run unit tests (forge + cargo)"
	@echo "  make test-scripts       Run script-level startup preflight tests"
	@echo "  make e2e                Run E2E test (send + watch)"
	@echo "  make send               Send a test message (MSG=\"hello\")"
	@echo "  make status-msg         Quick status check across operators"
	@echo "  make watch              Watch message lifecycle (GUID=0x...)"
	@echo ""
	@echo "Configuration:"
	@echo "  make configure          Regenerate configs from templates"
	@echo "  make addresses          Generate addresses.env from deploy data"
	@echo "  make refresh-epoch      Force-refresh settlement epoch for local devnet"
	@echo "  make reset-runtime      Reset runtime state (redis/relayer/sidecars) for deterministic restart"
	@echo "  make deploy-ccv-contracts Deploy SymbioticCCV source/dest contracts"
	@echo "  make configure-ccv-contracts Configure SymbioticCCV remote-chain caller rules"
	@echo ""
	@echo "Logs:"
	@echo "  make logs-operators     Follow all 3 operator logs"
	@echo "  make logs-operator-1    Follow operator-1 logs"
	@echo "  make logs-operator-2    Follow operator-2 logs"
	@echo "  make logs-operator-3    Follow operator-3 logs"
	@echo "  make logs-monitor       Follow oz-monitor logs"
	@echo "  make logs-relayer       Follow oz-relayer logs"
	@echo "  make logs-relays        Follow symbiotic-relay-1/2/3 logs"
	@echo ""
	@echo "Utilities:"
	@echo "  make status             Show running containers and health"
	@echo "  make help               Show this help message"

# ═══════════════════════════════════════════════════════════════════════════════
# PRIMARY COMMANDS
# ═══════════════════════════════════════════════════════════════════════════════

install:
	@echo "Installing dependencies..."
	cd contracts && npm install
	@echo "Dependencies installed."

start:
	@$(MAKE) ensure-env
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) PRIVATE_KEY=$(PRIVATE_KEY) ./scripts/start-stack.sh

stop:
	@echo "Stopping all containers (preserving state)..."
	docker compose --profile dev --profile infra down
	@echo "Stopped. Run 'make start' to resume."

clean:
	@echo "Full reset: stopping containers and removing data..."
	docker compose --profile dev --profile infra down -v
	rm -rf data/
	@echo "Cleaned. Run 'make start' for fresh start."

# ═══════════════════════════════════════════════════════════════════════════════
# CONFIGURATION
# ═══════════════════════════════════════════════════════════════════════════════

configure:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/generate-configs.sh
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/generate-addresses.sh
	@echo "✓ Configuration complete"

addresses:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/generate-addresses.sh

ensure-env:
	@./scripts/ensure-env.sh

refresh-epoch:
	@./scripts/refresh-epoch.sh

reset-runtime:
	@./scripts/reset-runtime-state.sh

configure-ccv-contracts:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) PRIVATE_KEY=$(PRIVATE_KEY) ./scripts/configure-ccv-contracts.sh

deploy-ccv-contracts:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) PRIVATE_KEY=$(PRIVATE_KEY) ./scripts/deploy-ccv-contracts.sh

shell:
	@if [ ! -f data/deploy-data/addresses.env ]; then \
		echo "ERROR: addresses.env not found. Run 'make start' first."; \
		exit 1; \
	fi
	@bash -lc 'set -a; \
		[ -f ./.env ] && source ./.env; \
		[ -f data/deploy-data/addresses.env ] && source data/deploy-data/addresses.env; \
		set +a; \
		echo ""; \
		echo "═══════════════════════════════════════════════════════════════════"; \
		echo "Loaded .env + data/deploy-data/addresses.env"; \
		echo "═══════════════════════════════════════════════════════════════════"; \
		echo ""; \
		echo "Available variables:"; \
		echo "  \$$DVN_SOURCE_ADDRESS      \$$DVN_DEST_ADDRESS"; \
		echo "  \$$CCV_SOURCE_ADDRESS      \$$CCV_DEST_ADDRESS"; \
		echo "  \$$CCV_SOURCE_ONRAMP_ADDRESS \$$CCV_DEST_OFFRAMP_ADDRESS"; \
		echo "  \$$TEST_OAPP_SOURCE_ADDRESS  \$$TEST_OAPP_DEST_ADDRESS"; \
		echo "  \$$SOURCE_RPC_URL          \$$DEST_RPC_URL"; \
		echo ""; \
		exec $$SHELL'

# ═══════════════════════════════════════════════════════════════════════════════
# TESTING
# ═══════════════════════════════════════════════════════════════════════════════

# Send a test message
# Usage: make send [MSG="hello world"]
send:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/msg send --message "$(if $(MSG),$(MSG),hello)"

# Watch message lifecycle until verified
# Usage: make watch [GUID=0x...] [TX=0x...] [TIMEOUT=120]
watch:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/msg watch \
		$(if $(GUID),--guid $(GUID)) \
		$(if $(TX),--tx $(TX)) \
		$(if $(TIMEOUT),--timeout $(TIMEOUT))

# Quick status check across all operators
# Usage: make status-msg [GUID=0x...]
status-msg:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/msg status $(if $(GUID),--guid $(GUID)) $(if $(TX),--tx $(TX))

# Alias for backwards compatibility
msg-status: status-msg

# Full E2E test: send message and watch until verified
# Usage: make e2e [MSG="hello"] [TIMEOUT=120] [VERBOSE=1]
e2e:
	@ROOT_CONFIG_FILE=$(ROOT_CONFIG_FILE) ./scripts/msg e2e \
		--message "$(if $(MSG),$(MSG),hello from e2e)" \
		$(if $(TIMEOUT),--timeout $(TIMEOUT)) \
		$(if $(VERBOSE),--verbose)

# ═══════════════════════════════════════════════════════════════════════════════
# SERVICE RESTARTS
# ═══════════════════════════════════════════════════════════════════════════════

restart-operators:
	@echo "Rebuilding and restarting all operators..."
	docker compose --profile dev up -d --build --force-recreate operator-1 operator-2 operator-3

restart-monitor:
	@echo "Restarting oz-monitor..."
	docker compose --profile dev restart oz-monitor

restart-relayer:
	@echo "Restarting oz-relayer..."
	docker compose --profile dev restart oz-relayer

restart-relays:
	@echo "Restarting symbiotic-relay-1, symbiotic-relay-2, and symbiotic-relay-3..."
	docker compose --profile dev restart symbiotic-relay-1 symbiotic-relay-2 symbiotic-relay-3

# ═══════════════════════════════════════════════════════════════════════════════
# DEVELOPMENT
# ═══════════════════════════════════════════════════════════════════════════════

dev-operator:
	@echo "Running operator-1 locally (services must be running in Docker)..."
	@echo "Tip: Run 'make start' first, then use this for fast iteration."
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found. Run 'make setup' first."; \
		exit 1; \
	fi
	@if [ ! -f data/generated-config/operator-1/config.json ]; then \
		echo "ERROR: Config not generated. Run 'make start' or 'make configure' first."; \
		exit 1; \
	fi
	@set -a && . ./.env && set +a && \
	cd operator && \
	RUST_LOG=debug \
	cargo run -- --config ../data/generated-config/operator-1/config.json

rebuild-operators:
	@echo "Rebuilding operator Docker image from scratch..."
	docker compose --profile dev build --no-cache operator-1
	docker compose --profile dev up -d --force-recreate operator-1 operator-2 operator-3
	@echo "All operators rebuilt and restarted."

# Run unit tests (contracts + operator)
test: test-contracts test-scripts
	@echo ""
	@echo "All unit tests passed!"

# Run contract tests only
test-contracts:
	@echo "Running contract tests..."
	cd contracts && forge test

test-scripts:
	@echo "Running script tests..."
	@bash scripts/tests/test-preflight-start.sh
	@bash scripts/tests/test-reset-runtime.sh
	@bash scripts/tests/test-make-root-config-propagation.sh
	@bash scripts/tests/test-generate-configs-layerzero-root-contract.sh
	@bash scripts/tests/test-start-stack-layerzero-eid-propagation.sh
	@bash scripts/tests/test-chainlink-ccv-msg-epoch-refresh.sh
	@echo "Script tests passed."

setup:
	@echo "Setting up environment..."
	./scripts/setup.sh
	@echo ""
	@echo "Setup complete! Now run: make start"

# ═══════════════════════════════════════════════════════════════════════════════
# LOGS
# ═══════════════════════════════════════════════════════════════════════════════

logs-operators:
	docker compose --profile dev logs -f operator-1 operator-2 operator-3

logs-operator-1:
	docker logs operator-1 -f

logs-operator-2:
	docker logs operator-2 -f

logs-operator-3:
	docker logs operator-3 -f

logs-monitor:
	docker logs oz-monitor -f

logs-relayer:
	docker logs oz-relayer -f

logs-relays:
	docker compose --profile dev logs -f symbiotic-relay-1 symbiotic-relay-2 symbiotic-relay-3

# ═══════════════════════════════════════════════════════════════════════════════
# UTILITIES
# ═══════════════════════════════════════════════════════════════════════════════

status:
	@if [ -f .env ]; then set -a && . ./.env && set +a; fi && \
	echo "═══════════════════════════════════════════════════════════════════" && \
	echo "Container Status" && \
	echo "═══════════════════════════════════════════════════════════════════" && \
	(docker ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" | grep -E "(anvil|operator|oz-monitor|oz-relayer|redis|symbiotic-relay)" || echo "No containers running") && \
	echo "" && \
	echo "═══════════════════════════════════════════════════════════════════" && \
	echo "Health Checks" && \
	echo "═══════════════════════════════════════════════════════════════════" && \
	(printf "operator-1:        " && curl -sf http://localhost:3001/healthz >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "operator-2:        " && curl -sf http://localhost:3002/healthz >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "operator-3:        " && curl -sf http://localhost:3003/healthz >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "oz-relayer:        " && curl -sf http://localhost:8080/api/v1/health -H "Authorization: Bearer $${OZ_RELAYER_API_KEY}" >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "symbiotic-relay-1: " && curl -sf http://localhost:8081/healthz >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "symbiotic-relay-2: " && curl -sf http://localhost:8082/healthz >/dev/null && echo "OK" || echo "NOT RUNNING") && \
	(printf "symbiotic-relay-3: " && curl -sf http://localhost:8083/healthz >/dev/null && echo "OK" || echo "NOT RUNNING")
	@echo ""
	@echo "═══════════════════════════════════════════════════════════════════"
	@echo "Deployment Status"
	@echo "═══════════════════════════════════════════════════════════════════"
	@ACTIVE_PROVIDER=$$(jq -er '.active_provider' $(ROOT_CONFIG_FILE) 2>/dev/null) || { \
		echo "Contracts: UNKNOWN (invalid or missing .active_provider in $(ROOT_CONFIG_FILE))"; \
		exit 1; \
	}; \
	if [ ! -f data/deploy-data/deploy-state.json ]; then \
		echo "Contracts: NOT DEPLOYED for '$$ACTIVE_PROVIDER' (missing data/deploy-data/deploy-state.json; run 'make start')"; \
	elif [ "$$ACTIVE_PROVIDER" = "layerzero" ] && jq -e '.providers.layerzero.source.dvn and .providers.layerzero.destination.dvn and .providers.layerzero.source.test_oapp and .providers.layerzero.destination.test_oapp' data/deploy-data/deploy-state.json >/dev/null 2>&1; then \
		echo "Contracts: DEPLOYED ($$ACTIVE_PROVIDER)"; \
		if [ -f data/deploy-data/addresses.env ]; then \
			cat data/deploy-data/addresses.env; \
		fi; \
	elif [ "$$ACTIVE_PROVIDER" = "chainlink_ccv" ] && jq -e '.providers.chainlink_ccv.source.ccv and .providers.chainlink_ccv.destination.ccv and .providers.chainlink_ccv.source.on_ramp and .providers.chainlink_ccv.destination.off_ramp' data/deploy-data/deploy-state.json >/dev/null 2>&1; then \
		echo "Contracts: DEPLOYED ($$ACTIVE_PROVIDER)"; \
		if [ -f data/deploy-data/addresses.env ]; then \
			cat data/deploy-data/addresses.env; \
		fi; \
	elif [ "$$ACTIVE_PROVIDER" != "layerzero" ] && [ "$$ACTIVE_PROVIDER" != "chainlink_ccv" ]; then \
		echo "Contracts: UNKNOWN (unsupported active_provider '$$ACTIVE_PROVIDER')"; \
		exit 1; \
	else \
		echo "Contracts: NOT DEPLOYED for '$$ACTIVE_PROVIDER' (deploy state incomplete; run 'make start')"; \
	fi
