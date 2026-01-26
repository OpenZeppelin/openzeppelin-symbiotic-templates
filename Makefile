.PHONY: help start stop clean install
.PHONY: restart-operators restart-monitor restart-relayer restart-relays
.PHONY: dev-operator rebuild-operators test
.PHONY: logs-operators logs-operator-1 logs-operator-2 logs-operator-3
.PHONY: logs-monitor logs-relayer logs-relays
.PHONY: status setup configure addresses shell
.PHONY: send watch msg-status

# Default private key for anvil (account 0)
PRIVATE_KEY ?= 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

# Marker file that indicates deployment is complete
MARKER_FILE := data/deploy-data/relay-infra-complete.marker

# ═══════════════════════════════════════════════════════════════════════════════
# HELP
# ═══════════════════════════════════════════════════════════════════════════════

help:
	@echo "Operator - Development Commands (3-Operator Architecture)"
	@echo "═══════════════════════════════════════════════════════════════════"
	@echo ""
	@echo "Primary Commands:"
	@echo "  make install            Install dependencies (contracts npm packages)"
	@echo "  make start              Smart start (deploys if needed, starts all)"
	@echo "  make stop               Stop all containers (preserve state)"
	@echo "  make clean              Full reset (stop + remove volumes + markers)"
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
	@echo "  make setup              Generate .env with operator keys"
	@echo "  make shell              Interactive shell with addresses loaded"
	@echo ""
	@echo "Testing:"
	@echo "  make test               Run automated E2E test"
	@echo "  make send               Send a test message (MSG=\"hello\")"
	@echo "  make watch              Watch message lifecycle (GUID=0x...)"
	@echo "  make msg-status         Quick status check across operators"
	@echo ""
	@echo "Configuration:"
	@echo "  make configure          Regenerate configs from templates"
	@echo "  make addresses          Generate addresses.env from deploy data"
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
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found. Run 'make setup' first."; \
		exit 1; \
	fi
	@if [ -f $(MARKER_FILE) ]; then \
		echo "═══ Contracts already deployed, regenerating configs... ═══"; \
		$(MAKE) configure; \
		echo "Starting services..."; \
		docker compose --profile dev up -d --remove-orphans >/dev/null 2>&1; \
	else \
		echo "═══ First run: full deployment ═══"; \
		echo ""; \
		echo "[1/6] Building + starting chains (parallel)..."; \
		( cd contracts && forge build --quiet && echo "      ✓ Contracts compiled" ) & \
		( docker compose --profile dev build --quiet operator-1 >/dev/null 2>&1 && echo "      ✓ Operator image built" ) & \
		( docker compose --profile infra up -d --remove-orphans >/dev/null 2>&1 && echo "      ✓ Chains starting" ) & \
		wait; \
		echo ""; \
		echo "[2/6] Waiting for chains..."; \
		( \
			timeout=30; elapsed=0; \
			while ! cast client --rpc-url http://localhost:8545 >/dev/null 2>&1; do \
				sleep 1; elapsed=$$((elapsed + 1)); \
				if [ $$elapsed -ge $$timeout ]; then echo "      ERROR: Timeout waiting for anvil"; exit 1; fi; \
			done; \
			echo "      ✓ anvil ready" \
		) & \
		( \
			timeout=30; elapsed=0; \
			while ! cast client --rpc-url http://localhost:8546 >/dev/null 2>&1; do \
				sleep 1; elapsed=$$((elapsed + 1)); \
				if [ $$elapsed -ge $$timeout ]; then echo "      ERROR: Timeout waiting for anvil-settlement"; exit 1; fi; \
			done; \
			echo "      ✓ anvil-settlement ready" \
		) & \
		wait || exit 1; \
		echo ""; \
		echo "[3/6] Deploying contracts..."; \
		mkdir -p data/deploy-data contracts/deploy-data; \
		cd contracts && \
		echo "      Phase 1: LayerZero + Relay infra..." && \
		forge script script/DeployLayerZero.s.sol:DeployLayerZero \
			--sig "deploySource()" \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ LayerZero source" && \
		forge script script/DeployLayerZero.s.sol:DeployLayerZero \
			--sig "deployDest()" \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ LayerZero dest" && \
		forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--code-size-limit 50000 \
			--gas-estimate-multiplier 150 \
			--slow \
			--quiet && \
		echo "        ✓ Relay infra (includes real Settlement)" && \
		echo "      Phase 2: DVN (needs LZ + Settlement addresses)..." && \
		SEND_ULN=$$(jq -r '.sendUln' deploy-data/layerzero_source.json) && \
		RECEIVE_ULN=$$(jq -r '.receiveUln' deploy-data/layerzero_dest.json) && \
		SETTLEMENT_ADDR=$$(jq -r '.settlement' deploy-data/relay_infra.json) && \
		forge script script/DeployDVN.s.sol:DeployDVN \
			--sig "deploySource(address)" $$SEND_ULN \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ DVN source" && \
		forge script script/DeployDVN.s.sol:DeployDVN \
			--sig "deployDest(address,address)" $$RECEIVE_ULN $$SETTLEMENT_ADDR \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ DVN dest" && \
		echo "      Phase 3: Configure ULN with DVN..." && \
		SRC_DVN=$$(jq -r '.dvn' deploy-data/source_contracts.json) && \
		DST_DVN=$$(jq -r '.dvn' deploy-data/dest_contracts.json) && \
		forge script script/DeployLayerZero.s.sol:DeployLayerZero \
			--sig "configureSource(address)" $$SRC_DVN \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ Source ULN configured" && \
		forge script script/DeployLayerZero.s.sol:DeployLayerZero \
			--sig "configureDest(address)" $$DST_DVN \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ Dest ULN configured" && \
		echo "      Phase 4: TestOApp..." && \
		forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
			--sig "deploySourceFromJson()" \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ TestOApp source" && \
		forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
			--sig "deployDestFromJson()" \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ TestOApp dest" && \
		forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
			--sig "configurePeersFromJson()" \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ Source peers configured" && \
		forge script script/examples/DeployTestOApp.s.sol:DeployTestOApp \
			--sig "configurePeersFromJson()" \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "        ✓ Dest peers configured" && \
		cd ..; \
		cp contracts/deploy-data/*.json data/deploy-data/; \
		date > data/deploy-data/deployment-complete.marker; \
		date > $(MARKER_FILE); \
		echo ""; \
		echo "      Mining blocks to finalize deposits..."; \
		cast rpc evm_mine --rpc-url http://localhost:8545 >/dev/null 2>&1; \
		cast rpc evm_mine --rpc-url http://localhost:8546 >/dev/null 2>&1; \
		echo "      ✓ Blocks mined"; \
		echo ""; \
		echo "[4/6] Generating genesis valset..."; \
		./scripts/generate-genesis.sh && \
		echo "      ✓ Genesis committed"; \
		echo ""; \
		echo "[5/6] Generating configs..."; \
		$(MAKE) configure; \
		echo ""; \
		echo "[6/6] Starting services..."; \
		docker compose --profile dev up -d --remove-orphans >/dev/null 2>&1; \
		echo "      ✓ All services started"; \
	fi
	@echo ""
	@echo "═══════════════════════════════════════════════════════════════════"
	@echo "Stack started! Run 'make status' to check health."
	@echo "═══════════════════════════════════════════════════════════════════"

stop:
	@echo "Stopping all containers (preserving state)..."
	docker compose --profile dev --profile infra down
	@echo "Stopped. Run 'make start' to resume."

clean:
	@echo "Full reset: stopping containers and removing data..."
	docker compose --profile dev --profile infra down -v
	rm -rf data/
	@echo "Cleaned. Run 'make setup && make start' for fresh start."

# ═══════════════════════════════════════════════════════════════════════════════
# CONFIGURATION
# ═══════════════════════════════════════════════════════════════════════════════

configure:
	@if [ ! -f $(MARKER_FILE) ]; then \
		echo "ERROR: Contracts not deployed. Run 'make start' first."; \
		exit 1; \
	fi
	@./scripts/generate-configs.sh
	@./scripts/generate-addresses.sh
	@echo "✓ Configuration complete"

addresses:
	@if [ ! -f $(MARKER_FILE) ]; then \
		echo "ERROR: Contracts not deployed. Run 'make start' first."; \
		exit 1; \
	fi
	@./scripts/generate-addresses.sh

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
		echo "  \$$TEST_OAPP_SOURCE_ADDRESS  \$$TEST_OAPP_DEST_ADDRESS"; \
		echo "  \$$SOURCE_RPC_URL          \$$DEST_RPC_URL"; \
		echo ""; \
		exec $$SHELL'

# ═══════════════════════════════════════════════════════════════════════════════
# TESTING
# ═══════════════════════════════════════════════════════════════════════════════

send:
	@if [ ! -f $(MARKER_FILE) ]; then \
		echo "ERROR: Contracts not deployed. Run 'make start' first."; \
		exit 1; \
	fi
	@./scripts/send-message.sh "$(MSG)"

watch:
	@if [ ! -f $(MARKER_FILE) ]; then \
		echo "ERROR: Contracts not deployed. Run 'make start' first."; \
		exit 1; \
	fi
	@if [ -n "$(GUID)" ]; then \
		GUID="$(GUID)" ./scripts/watch-message.sh; \
	elif [ -n "$(TX)" ]; then \
		TX="$(TX)" ./scripts/watch-message.sh; \
	else \
		./scripts/watch-message.sh; \
	fi

msg-status:
	@if [ -n "$(GUID)" ]; then \
		./scripts/msg-status.sh "$(GUID)"; \
	else \
		./scripts/msg-status.sh; \
	fi

# ═══════════════════════════════════════════════════════════════════════════════
# SERVICE RESTARTS
# ═══════════════════════════════════════════════════════════════════════════════

restart-operators:
	@echo "Rebuilding and restarting all operators..."
	docker compose --profile dev up -d --build operator-1 operator-2 operator-3

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
	docker compose --profile dev up -d operator-1 operator-2 operator-3
	@echo "All operators rebuilt and restarted."

test:
	@echo "Running E2E test: emit event and verify proof..."
	@if [ ! -f $(MARKER_FILE) ]; then \
		echo "ERROR: Contracts not deployed. Run 'make start' first."; \
		exit 1; \
	fi
	./scripts/test-e2e-symbiotic-relay.sh

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
	@if [ -f $(MARKER_FILE) ]; then \
		echo "Contracts: DEPLOYED"; \
		if [ -f data/deploy-data/addresses.env ]; then \
			cat data/deploy-data/addresses.env; \
		fi; \
	else \
		echo "Contracts: NOT DEPLOYED (run 'make start' to deploy)"; \
	fi
