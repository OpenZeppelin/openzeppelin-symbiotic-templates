.PHONY: help start stop clean install deploy validate refresh-genesis run-operators
.PHONY: restart-operators restart-monitor restart-relayer restart-relays
.PHONY: dev-operator rebuild-operators test test-contracts test-operator e2e
.PHONY: test-scripts
.PHONY: logs-operators logs-operator-1 logs-operator-2 logs-operator-3
.PHONY: logs-monitor logs-relayer logs-relays
.PHONY: status setup shell
.PHONY: send watch

# Environment selection: local (default), testnet, mainnet
ENV ?= local
ENV_CONFIG := config/environments/$(ENV).json
DEPLOYMENTS_FILE := deployments/$(ENV).json
GENERATED_DIR := generated/$(ENV)
XTASK = cargo xtask --env $(ENV) --env-config $(ENV_CONFIG) --deployments $(DEPLOYMENTS_FILE) --generated-dir $(GENERATED_DIR)

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
	@echo "  make deploy             Deploy contracts and generate runtime config"
	@echo "  make validate           Run read-only validation checks"
	@echo "  make refresh-genesis    Refresh committed settlement genesis"
	@echo "  make start              Start full local stack (local-chain envs only)"
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
	@if [ "$(_SOURCE_CHAIN_ID)" != "31337" ]; then \
		echo "ERROR: make start is local-only. Use 'make deploy ENV=$(ENV)' and 'make run-operators ENV=$(ENV)'."; \
		exit 1; \
	fi
	@./scripts/ensure-env.sh
	@$(XTASK) start-local

deploy:
	@if [ "$(_SOURCE_CHAIN_ID)" = "31337" ]; then ./scripts/ensure-env.sh; fi
	@$(XTASK) deploy

validate:
	@$(XTASK) validate

refresh-genesis:
	@$(XTASK) refresh-genesis

run-operators:
	@if [ "$(_SOURCE_CHAIN_ID)" = "31337" ]; then \
		echo "ERROR: use 'make start' for the full local stack."; \
		exit 1; \
	fi
	@$(XTASK) run-operators

stop:
	@echo "Stopping all containers (preserving state)..."
	docker compose $(COMPOSE_FILES) --profile dev --profile infra down
	@if [ "$(_SOURCE_CHAIN_ID)" = "31337" ]; then \
		echo "Stopped. Run 'make start' to resume."; \
	else \
		echo "Stopped. Run 'make run-operators ENV=$(ENV)' to restart non-local services."; \
	fi

clean:
	@$(XTASK) clean

# ═══════════════════════════════════════════════════════════════════════════════
# BOOTSTRAP
# ═══════════════════════════════════════════════════════════════════════════════

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
	@$(XTASK) msg send "$(if $(MSG),$(MSG),hello)"

# Watch message lifecycle until verified
# Usage: make watch [GUID=0x...] [TX=0x...] [TIMEOUT=120]
watch:
	@$(XTASK) msg watch \
		$(if $(GUID),--id $(GUID)) \
		$(if $(TX),--tx $(TX)) \
		$(if $(TIMEOUT),--timeout $(TIMEOUT))

# Full E2E test: send message and watch until verified
# Usage: make e2e [MSG="hello"] [TIMEOUT=120]
e2e:
	@$(XTASK) msg e2e "$(if $(MSG),$(MSG),hello from e2e)" $(if $(TIMEOUT),--timeout $(TIMEOUT))

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
	@if [ "$(_SOURCE_CHAIN_ID)" = "31337" ]; then \
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
	cd contracts && forge test --no-match-contract Integration

test-scripts:
	@echo "Running script tests..."
	@bash scripts/tests/test-make-root-config-propagation.sh
	@echo "Script tests passed."

setup:
	@echo "Setting up environment..."
	./scripts/setup.sh
	@echo ""
	@echo "Setup complete! Now run: make start ENV=$(ENV)"

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
	@$(XTASK) status
test-operator:
	@echo "Running operator tests..."
	cd operator && cargo test
