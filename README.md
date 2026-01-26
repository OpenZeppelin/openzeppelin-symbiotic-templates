# OpenZeppelin Symbiotic Templates

Templates for building cross-chain verification systems using [Symbiotic](https://symbiotic.fi/) shared security.

## LayerZero DVN Template

A Decentralized Verifier Network (DVN) for [LayerZero](https://layerzero.network/) secured by Symbiotic's BLS threshold signatures. The DVN verifies cross-chain messages using Merkle tree batching for gas efficiency.

### How it works

1. **Source chain**: A cross-chain message is sent, and the DVN is notified of a new verification job
2. **Off-chain**: Operators batch messages into a Merkle tree and collectively sign the root using BLS threshold signatures
3. **Destination chain**: The signed proof is submitted, the DVN verifies the quorum was met, and the message is delivered

### Local Development

The devnet runs a 3-operator setup to simulate quorum verification locally. Production deployments typically use a single operator.

## Prerequisites

- Docker and Docker Compose v2+
- [Foundry](https://book.getfoundry.sh/getting-started/installation) (forge, cast, anvil)
- [Rust/Cargo](https://rustup.rs/) (for `make dev-operator`)
- jq

## Quick Start

```bash
# Generate environment and operator keys
make setup

# Start the stack (builds, deploys contracts, starts services)
make start

# Check service health
make status

# Run end-to-end test
make test
```

## Commands

```
make setup              Generate .env with operator keys
make start              Smart start (deploys if needed, starts all)
make stop               Stop all (preserve state)
make clean              Full reset (removes volumes and deployed state)

make restart-operators  Rebuild and restart operators
make restart-monitor    Restart event monitor
make restart-relayer    Restart transaction relayer
make restart-relays     Restart BLS signing sidecars

make dev-operator       Run operator locally with cargo
make test               Emit event and verify proof end-to-end

make logs-operators     Follow all operator logs
make logs-monitor       Follow monitor logs
make logs-relayer       Follow relayer logs
make status             Show container and health status
```

## Project Structure

```
├── contracts/          # Solidity contracts (Foundry)
│   ├── src/           # DVN, Settlement, and supporting contracts
│   └── script/        # Deployment scripts
├── operator/          # Rust operator service
├── config/            # Service configurations
│   ├── operator-*/    # Per-operator configs
│   ├── oz-monitor/    # Event monitoring
│   └── oz-relayer/    # Transaction submission
├── scripts/           # Automation scripts
└── docker-compose.yml # Service orchestration
```

## Services

| Service               | Port      | Description                   |
| --------------------- | --------- | ----------------------------- |
| anvil                 | 8545      | Source chain (ID: 31337)      |
| anvil-settlement      | 8546      | Destination chain (ID: 31338) |
| operator-1/2/3        | 3001-3003 | Operator HTTP APIs            |
| symbiotic-relay-1/2/3 | 8081-8083 | BLS signing sidecars          |
| oz-monitor            | -         | Event watching                |
| oz-relayer            | 8080      | Transaction submission        |
| redis                 | 6379      | Job queue                     |

## Documentation

- [Configuration](docs/configuration.md) - Environment variables, operator config, webhooks, retry settings
- [API Reference](docs/api-reference.md) - HTTP endpoints for webhooks, debugging, and proofs
- [Troubleshooting](docs/troubleshooting.md) - Common issues, debugging, log analysis
- [Architecture](docs/architecture.md) - System overview, message flow, BLS signing

## Contract Addresses

After deployment, addresses are written to `data/deploy-data/`:

- `source_contracts.json` - DVN on source chain
- `dest_contracts.json` - DVN on destination chain
- `relay_infra.json` - Symbiotic relay infrastructure (includes Settlement)
- `addresses.env` - All addresses in shell-sourceable format

For manual testing, source the addresses file:

```bash
source data/deploy-data/addresses.env
echo "DVN Source: $DVN_SOURCE_ADDRESS"
echo "TestOApp:   $TEST_OAPP_SOURCE_ADDRESS"
```

## Contributing

Contributions are welcome. Please open an issue to discuss significant changes before submitting a PR.

## License

MIT
