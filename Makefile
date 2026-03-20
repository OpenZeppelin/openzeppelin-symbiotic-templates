.PHONY: help start stop clean install deploy validate run-operators
.PHONY: restart-operators restart-monitor restart-relayer restart-relays
.PHONY: dev-operator rebuild-operators test test-contracts test-operator e2e
.PHONY: test-scripts
.PHONY: logs-operators logs-operator-1 logs-operator-2 logs-operator-3
.PHONY: logs-monitor logs-relayer logs-relays
.PHONY: status setup configure publish-addresses shell ensure-env refresh-epoch reset-runtime
.PHONY: send watch msg-status
.PHONY: deploy-ccv-contracts configure-ccv-contracts

# Environment selection: local (default), testnet, mainnet
ENV ?= local
ENV_CONFIG := config/environments/$(ENV).json
DEPLOYMENTS_FILE := deployments/$(ENV).json
GENERATED_DIR := generated/$(ENV)

# Detect local mode from environment config (anvil chain ID = 31337)
_SOURCE_CHAIN_ID := $(shell jq -r '.chains.source.chainId // empty' $(ENV_CONFIG) 2>/dev/null)
ifeq ($(_SOURCE_CHAIN_ID),31337)
  COMPOSE_FILES := -f docker-compose.yml -f docker-compose.local.yml
  PRIVATE_KEY ?= 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
else
  COMPOSE_FILES :=
  PRIVATE_KEY ?=
endif

# ═══════════════════════════════════════════════════════════════════════════════
# HELP
# ═══════════════════════════════════════════════════════════════════════════════

help:
	@echo "Operator - Commands"
	@echo "═══════════════════════════════════════════════════════════════════"
	@echo ""
	@echo "Environment: ENV=$(ENV)"
	@echo "  env config:   $(ENV_CONFIG)"
	@echo "  deployments:  $(DEPLOYMENTS_FILE)"
	@echo "  generated:    $(GENERATED_DIR)"
	@echo ""
	@echo "Primary Commands:"
	@echo "  make deploy             Deploy/reconcile contracts and generated config"
	@echo "  make validate           Run read-only validation checks"
	@echo "  make start              Start full local stack (ENV=local only)"
	@echo "  make run-operators      Start non-local operator services (requires deploy)"
	@echo "  make install            Install dependencies (contracts npm packages)"
	@echo "  make stop               Stop all containers (preserve state)"
	@echo "  make clean              Reset local/generated runtime state"
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
	@if [ "$(ENV)" != "local" ]; then \
		echo "ERROR: make start is local-only. Use 'make deploy ENV=$(ENV)' and 'make run-operators ENV=$(ENV)'."; \
		exit 1; \
	fi
	@$(MAKE) ensure-env
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) GENERATED_DIR=$(GENERATED_DIR) $(if $(PRIVATE_KEY),PRIVATE_KEY=$(PRIVATE_KEY)) COMPOSE_FILES="$(COMPOSE_FILES)" STACK_MODE=full ./scripts/start-stack.sh

deploy:
	@if [ "$(ENV)" = "local" ]; then $(MAKE) ensure-env; fi
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) GENERATED_DIR=$(GENERATED_DIR) $(if $(PRIVATE_KEY),PRIVATE_KEY=$(PRIVATE_KEY)) COMPOSE_FILES="$(COMPOSE_FILES)" STACK_MODE=deploy_only ./scripts/start-stack.sh

validate:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) GENERATED_DIR=$(GENERATED_DIR) ./scripts/validate-env.sh

run-operators:
	@if [ "$(ENV)" = "local" ]; then \
		echo "ERROR: use 'make start' for the full local stack."; \
		exit 1; \
	fi
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) GENERATED_DIR=$(GENERATED_DIR) VALIDATE_MANAGED_OPERATORS=1 $(if $(PRIVATE_KEY),PRIVATE_KEY=$(PRIVATE_KEY)) COMPOSE_FILES="$(COMPOSE_FILES)" STACK_MODE=services_only ./scripts/start-stack.sh

stop:
	@echo "Stopping all containers (preserving state)..."
	docker compose $(COMPOSE_FILES) --profile dev --profile infra down
	@if [ "$(ENV)" = "local" ]; then \
		echo "Stopped. Run 'make start' to resume."; \
	else \
		echo "Stopped. Run 'make run-operators ENV=$(ENV)' to restart non-local services."; \
	fi

clean:
	@echo "Resetting generated/local runtime state..."
	-docker compose $(COMPOSE_FILES) --profile dev --profile infra down -v --remove-orphans 2>/dev/null
	rm -rf data/ generated/
	@if [ "$(ENV)" = "local" ]; then rm -f $(DEPLOYMENTS_FILE); fi
	@if [ "$(ENV)" = "local" ]; then \
		echo "Cleaned. Run 'make deploy' or 'make start'."; \
	else \
		echo "Cleaned. Run 'make deploy ENV=$(ENV)' or 'make run-operators ENV=$(ENV)'."; \
	fi

# ═══════════════════════════════════════════════════════════════════════════════
# CONFIGURATION
# ═══════════════════════════════════════════════════════════════════════════════

publish-addresses:
	@ENV_CONFIG=$(ENV_CONFIG) ./scripts/publish-addresses.sh

configure:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) GENERATED_DIR=$(GENERATED_DIR) bash -c 'source scripts/lib/common.sh && generate_oz_configs'
	@echo "✓ OZ configs generated"

ensure-env:
	@./scripts/ensure-env.sh

refresh-epoch:
	@COMPOSE_FILES="$(COMPOSE_FILES)" ./scripts/refresh-epoch.sh

reset-runtime:
	@COMPOSE_FILES="$(COMPOSE_FILES)" ./scripts/reset-runtime-state.sh

configure-ccv-contracts:
	@ENV_CONFIG=$(ENV_CONFIG) PRIVATE_KEY=$(PRIVATE_KEY) ./scripts/configure-ccv-contracts.sh

deploy-ccv-contracts:
	@ENV_CONFIG=$(ENV_CONFIG) PRIVATE_KEY=$(PRIVATE_KEY) ./scripts/deploy-ccv-contracts.sh

shell:
	@if [ ! -f $(ENV_CONFIG) ]; then \
		echo "ERROR: Environment config not found: $(ENV_CONFIG)."; \
		exit 1; \
	fi
	@bash -lc 'set -a; \
		[ -f ./.env ] && source ./.env; \
		export ENV_CONFIG=$(ENV_CONFIG); \
		export DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE); \
		set +a; \
		echo ""; \
		echo "═══════════════════════════════════════════════════════════════════"; \
		echo "Loaded .env + ENV_CONFIG=$(ENV_CONFIG) + DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE)"; \
		echo "═══════════════════════════════════════════════════════════════════"; \
		echo ""; \
		echo "Environment: jq . $(ENV_CONFIG)"; \
		echo "Deployments: [ -f $(DEPLOYMENTS_FILE) ] && jq . $(DEPLOYMENTS_FILE) || echo missing"; \
		echo ""; \
		exec $$SHELL'

# ═══════════════════════════════════════════════════════════════════════════════
# TESTING
# ═══════════════════════════════════════════════════════════════════════════════

# Send a test message
# Usage: make send [MSG="hello world"]
send:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) ./scripts/msg send --message "$(if $(MSG),$(MSG),hello)"

# Watch message lifecycle until verified
# Usage: make watch [GUID=0x...] [TX=0x...] [TIMEOUT=120]
watch:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) ./scripts/msg watch \
		$(if $(GUID),--guid $(GUID)) \
		$(if $(TX),--tx $(TX)) \
		$(if $(TIMEOUT),--timeout $(TIMEOUT))

# Quick status check across all operators
# Usage: make status-msg [GUID=0x...]
status-msg:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) ./scripts/msg status $(if $(GUID),--guid $(GUID)) $(if $(TX),--tx $(TX))

# Alias for backwards compatibility
msg-status: status-msg

# Full E2E test: send message and watch until verified
# Usage: make e2e [MSG="hello"] [TIMEOUT=120] [VERBOSE=1]
e2e:
	@ENV=$(ENV) ENV_CONFIG=$(ENV_CONFIG) DEPLOYMENTS_FILE=$(DEPLOYMENTS_FILE) ./scripts/msg e2e \
		--message "$(if $(MSG),$(MSG),hello from e2e)" \
		$(if $(TIMEOUT),--timeout $(TIMEOUT)) \
		$(if $(VERBOSE),--verbose)

# ═══════════════════════════════════════════════════════════════════════════════
# SERVICE RESTARTS
# ═══════════════════════════════════════════════════════════════════════════════

restart-operators:
	@echo "Rebuilding and restarting all operators..."
	docker compose $(COMPOSE_FILES) --profile dev up -d --no-deps --build --force-recreate operator-1 operator-2 operator-3

restart-monitor:
	@echo "Restarting oz-monitor..."
	docker compose $(COMPOSE_FILES) --profile dev restart oz-monitor

restart-relayer:
	@echo "Restarting oz-relayer..."
	docker compose $(COMPOSE_FILES) --profile dev restart oz-relayer

restart-relays:
	@echo "Restarting symbiotic-relay-1, symbiotic-relay-2, and symbiotic-relay-3..."
	docker compose $(COMPOSE_FILES) --profile dev restart symbiotic-relay-1 symbiotic-relay-2 symbiotic-relay-3

# ═══════════════════════════════════════════════════════════════════════════════
# DEVELOPMENT
# ═══════════════════════════════════════════════════════════════════════════════

dev-operator:
	@echo "Running operator-1 locally (services must be running in Docker)..."
	@if [ "$(ENV)" = "local" ]; then \
		echo "Tip: Run 'make start' first, then use this for fast iteration."; \
	else \
		echo "Tip: Run 'make run-operators ENV=$(ENV)' first, then use this for fast iteration."; \
	fi
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found. Run 'make setup' first."; \
		exit 1; \
	fi
	@if [ ! -f $(ENV_CONFIG) ]; then \
		echo "ERROR: Environment config not found: $(ENV_CONFIG)"; \
		exit 1; \
	fi
	@set -a && . ./.env && set +a && \
	cd operator && \
	RUST_LOG=debug \
	cargo run -- --environment ../$(ENV_CONFIG) --deployments ../$(DEPLOYMENTS_FILE) --operator-index 1

rebuild-operators:
	@echo "Rebuilding operator Docker image from scratch..."
	docker compose $(COMPOSE_FILES) --profile dev build --no-cache operator-1
	docker compose $(COMPOSE_FILES) --profile dev up -d --no-deps --force-recreate operator-1 operator-2 operator-3
	@echo "All operators rebuilt and restarted."

# Run unit tests (contracts + operator)
test: test-contracts test-scripts test-operator
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
	docker compose $(COMPOSE_FILES) --profile dev logs -f operator-1 operator-2 operator-3

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
	docker compose $(COMPOSE_FILES) --profile dev logs -f symbiotic-relay-1 symbiotic-relay-2 symbiotic-relay-3

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
	@ACTIVE_PROVIDER=$$(jq -er '.activeProvider' $(ENV_CONFIG) 2>/dev/null) || { \
		echo "Contracts: UNKNOWN (invalid or missing .activeProvider in $(ENV_CONFIG))"; \
		exit 1; \
	}; \
	SRC_DEPLOYS=$$(jq -r '.source | length' $(DEPLOYMENTS_FILE) 2>/dev/null || echo 0); \
	DST_DEPLOYS=$$(jq -r '.destination | length' $(DEPLOYMENTS_FILE) 2>/dev/null || echo 0); \
	if [ "$$SRC_DEPLOYS" -gt 0 ] && [ "$$DST_DEPLOYS" -gt 0 ]; then \
		echo "Contracts: DEPLOYED ($$ACTIVE_PROVIDER)"; \
		echo "  Source:"; \
		jq -r '.source | to_entries[] | "    \(.key): \(.value)"' $(DEPLOYMENTS_FILE) 2>/dev/null | head -5; \
		echo "  Destination:"; \
		jq -r '.destination | to_entries[] | "    \(.key): \(.value)"' $(DEPLOYMENTS_FILE) 2>/dev/null | head -5; \
	else \
		echo "Contracts: NOT DEPLOYED for '$$ACTIVE_PROVIDER' (run 'make deploy ENV=$(ENV)')"; \
	fi
test-operator:
	@echo "Running operator tests..."
	cd operator && cargo test
