.PHONY: help install build test clean deploy-local interact-local start-anvil stop-anvil deploy-layerzero emit-layerzero-job

# Default target
help:
	@echo "Available commands:"
	@echo "  make install            - Install Foundry dependencies"
	@echo "  make build              - Build the contracts"
	@echo "  make test               - Run tests"
	@echo "  make test-verbose       - Run tests with verbose output"
	@echo "  make clean              - Clean build artifacts"
	@echo "  make start-anvil        - Start local Anvil node in background"
	@echo "  make stop-anvil         - Stop local Anvil node"
	@echo "  make deploy-local       - Deploy CCIP contracts to local Anvil"
	@echo "  make interact-local     - Interact with deployed contract on Anvil"
	@echo "  make examples           - Run CCIP example scripts"
	@echo "  make coverage           - Generate test coverage report"
	@echo "  make gas-report         - Generate gas usage report"
	@echo ""
	@echo "LayerZero commands:"
	@echo "  make deploy-layerzero   - Deploy MockLayerZeroDVN to local Anvil"
	@echo "  make emit-layerzero-job - Emit a test JobAssigned event"

# Install dependencies
install:
	@echo "Installing Foundry dependencies..."
	forge install foundry-rs/forge-std --no-commit

# Build the contracts
build:
	@echo "Building contracts..."
	forge build

# Run tests
test:
	@echo "Running tests..."
	forge test

# Run tests with verbose output
test-verbose:
	@echo "Running tests (verbose)..."
	forge test -vvv

# Run tests with gas reporting
gas-report:
	@echo "Running tests with gas report..."
	forge test --gas-report

# Generate coverage report
coverage:
	@echo "Generating coverage report..."
	forge coverage

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	forge clean
	rm -rf cache out broadcast

# Start local Anvil node
start-anvil:
	@echo "Starting Anvil node..."
	@anvil --host 0.0.0.0 --port 8545 > anvil.log 2>&1 & echo $$! > .anvil.pid
	@sleep 2
	@echo "Anvil started with PID $$(cat .anvil.pid)"

# Stop local Anvil node
stop-anvil:
	@if [ -f .anvil.pid ]; then \
		kill $$(cat .anvil.pid) 2>/dev/null || true; \
		rm .anvil.pid; \
		echo "Anvil stopped"; \
	else \
		echo "Anvil is not running (no PID file found)"; \
	fi

# Deploy to local Anvil
deploy-local:
	@echo "Deploying to local Anvil..."
	@forge script script/Deploy.s.sol:DeployScript \
		--rpc-url http://localhost:8545 \
		--broadcast \
		--private-key $$PRIVATE_KEY
	@echo ""
	@echo "Contract deployed! Copy the address and set EMITTER_ADDRESS in .env"

# Interact with deployed contract
interact-local:
	@if [ -z "$$EMITTER_ADDRESS" ]; then \
		echo "Error: EMITTER_ADDRESS not set. Please set it in .env or environment"; \
		exit 1; \
	fi
	@echo "Interacting with contract at $$EMITTER_ADDRESS..."
	@forge script script/InteractWithEmitter.s.sol:InteractWithEmitterScript \
		--rpc-url http://localhost:8545 \
		--broadcast \
		--private-key $$PRIVATE_KEY

# Format code
format:
	@echo "Formatting code..."
	forge fmt

# Lint code
lint:
	@echo "Linting code..."
	forge fmt --check

# Run examples
examples:
	@if [ -z "$$EMITTER_ADDRESS" ]; then \
		echo "Error: EMITTER_ADDRESS not set. Please set it in .env or environment"; \
		exit 1; \
	fi
	@echo "Running example scripts on contract at $$EMITTER_ADDRESS..."
	@forge script script/Examples.s.sol:ExamplesScript \
		--rpc-url http://localhost:8545 \
		--broadcast \
		--private-key $$PRIVATE_KEY

# ============================================================================
# LayerZero Commands
# ============================================================================

# Deploy MockLayerZeroDVN to local Anvil
deploy-layerzero:
	@echo "Deploying MockLayerZeroDVN to local Anvil..."
	@forge script script/DeployLayerZero.s.sol:DeployLayerZeroScript \
		--rpc-url http://localhost:8545 \
		--broadcast \
		--private-key $$PRIVATE_KEY
	@echo ""
	@echo "Contract deployed! Set DVN_ADDRESS to the deployed address"

# Emit a test LayerZero JobAssigned event
emit-layerzero-job:
	@if [ -z "$$DVN_ADDRESS" ]; then \
		echo "Error: DVN_ADDRESS not set. Deploy first with 'make deploy-layerzero'"; \
		exit 1; \
	fi
	@echo "Emitting JobAssigned event from DVN at $$DVN_ADDRESS..."
	@forge script script/EmitLayerZeroJob.s.sol:EmitLayerZeroJobScript \
		--rpc-url http://localhost:8545 \
		--broadcast \
		--private-key $$PRIVATE_KEY

