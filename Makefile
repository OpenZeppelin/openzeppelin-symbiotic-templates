.PHONY: help start stop clean
.PHONY: restart-operators restart-monitor restart-relayer restart-relays
.PHONY: dev-operator rebuild-operators test
.PHONY: logs-operators logs-operator-1 logs-operator-2 logs-operator-3
.PHONY: logs-monitor logs-relayer logs-relays
.PHONY: status setup

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
	@echo "  make test               Emit event + verify proof"
	@echo "  make setup              Generate .env with operator keys"
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

start:
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found. Run 'make setup' first."; \
		exit 1; \
	fi
	@if [ -f $(MARKER_FILE) ]; then \
		echo "═══ Contracts already deployed, starting services... ═══"; \
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
		mkdir -p data/deploy-data; \
		cd contracts && \
		forge script script/DeployDVN.s.sol:DeployDVN \
			--sig "deploySource()" \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "      ✓ DVN source" && \
		forge script script/DeployDVN.s.sol:DeployDVN \
			--sig "deploySettlement()" \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "      ✓ Settlement" && \
		forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
			--rpc-url http://localhost:8545 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--code-size-limit 50000 \
			--quiet && \
		echo "      ✓ Relay infra" && \
		SETTLEMENT_ADDR=$$(sed -n 's/.*"settlement":[[:space:]]*"\(0x[^"]*\)".*/\1/p' deploy-data/settlement_contract.json) && \
		forge script script/DeployDVN.s.sol:DeployDVN \
			--sig "deployDest(address)" $$SETTLEMENT_ADDR \
			--rpc-url http://localhost:8546 \
			--broadcast \
			--private-key $(PRIVATE_KEY) \
			--quiet && \
		echo "      ✓ DVN dest" && \
		cd ..; \
		cp contracts/deploy-data/*.json data/deploy-data/; \
		date > data/deploy-data/deployment-complete.marker; \
		date > $(MARKER_FILE); \
		echo ""; \
		echo "[4/6] Generating genesis valset..."; \
		./scripts/generate-genesis.sh && \
		echo "      ✓ Genesis committed"; \
		echo ""; \
		echo "[5/6] Updating configs..."; \
		DVN_SRC=$$(jq -r '.dvn' data/deploy-data/source_contracts.json) && \
		DVN_DST=$$(jq -r '.dvn' data/deploy-data/dest_contracts.json) && \
		echo "      DVN Source: $$DVN_SRC" && \
		echo "      DVN Dest:   $$DVN_DST" && \
		for i in 1 2 3; do \
			sed -i '' "s|\$${DVN_DEST_ADDRESS}|$$DVN_DST|g" config/operator-$$i/config.json; \
		done && \
		sed -i '' "s|0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512|$$DVN_SRC|g" config/oz-monitor/monitors/layerzero_job_assigned.json 2>/dev/null || true && \
		jq --arg dvn "$$DVN_SRC" '.addresses[0].address = $$dvn' config/oz-monitor/monitors/layerzero_job_assigned.json > /tmp/monitor.json && \
		mv /tmp/monitor.json config/oz-monitor/monitors/layerzero_job_assigned.json && \
		echo "      ✓ Configs updated"; \
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
	docker compose --profile dev --profile deploy --profile infra down
	@echo "Stopped. Run 'make start' to resume."

clean:
	@echo "Full reset: stopping containers and removing data..."
	docker compose --profile dev --profile deploy --profile infra down -v
	rm -rf data/
	@echo "Cleaned. Run 'make setup && make start' for fresh start."

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
	@set -a && . ./.env && set +a && \
	cd operator && \
	RUST_LOG=debug \
	cargo run -- --config ../config/operator-1/config.json

rebuild-operators:
	@echo "Rebuilding operator Docker image from scratch..."
	docker compose --profile dev build --no-cache operator-1
	docker compose --profile dev up -d operator-1 operator-2 operator-3
	@echo "All operators rebuilt and restarted."

test:
	@echo "Running E2E test: emit event and verify proof..."
	@if [ ! -f data/deploy-data/deployment-complete.marker ]; then \
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
