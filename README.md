# OpenZeppelin Symbiotic Templates

Templates for building cross-chain verification systems using [Symbiotic](https://symbiotic.fi/) shared security.

## LayerZero DVN Template

A Decentralized Verifier Network (DVN) for [LayerZero](https://layerzero.network/) secured by Symbiotic's BLS threshold signatures. The DVN verifies cross-chain messages using Merkle tree batching for gas efficiency.

### How it works

1. **Source chain**: LayerZero's `SendUln302` calls the DVN contract, emitting a `JobAssigned` event
2. **Off-chain**: Operators receive the event, batch jobs into a Merkle tree, and sign the root with BLS keys
3. **Destination chain**: The aggregated signature and Merkle proof are submitted to the DVN, which verifies the quorum via Symbiotic's Settlement contract and forwards to LayerZero's `ReceiveUln302`

### Local Development

The devnet runs a 3-operator setup to simulate quorum verification locally. Production deployments typically use a single operator.

## Prerequisites

- Docker and Docker Compose v2+
- [Foundry](https://book.getfoundry.sh/getting-started/installation) (forge, cast, anvil)
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

| Service | Port | Description |
|---------|------|-------------|
| anvil | 8545 | Source chain (ID: 31337) |
| anvil-settlement | 8546 | Destination chain (ID: 31338) |
| operator-1/2/3 | 3001-3003 | Operator HTTP APIs |
| symbiotic-relay-1/2/3 | 8081-8083 | BLS signing sidecars |
| oz-relayer | 8080 | Transaction submission |
| redis | 6379 | Job queue |

## Configuration

### Environment

Run `make setup` to generate `.env`, or copy from `.env.example`:

| Variable | Description |
|----------|-------------|
| `PRIVATE_KEY` | Deployer key (default: Anvil account 0) |
| `LOG_LEVEL` | Logging level (debug, info, warn, error) |
| `API_KEY` | Webhook authentication |
| `OZ_RELAYER_API_KEY` | Relayer API authentication |
| `SIDECAR_*_SECRET_KEYS` | BLS keys per operator (generated) |

### Operator

Each operator reads from `config/operator-{n}/config.json`. Key settings:

| Section | Setting | Default | Description |
|---------|---------|---------|-------------|
| `symbiotic_relay` | `address` | - | BLS sidecar gRPC endpoint |
| `symbiotic_relay` | `max_retries` | 3 | Retry attempts for gRPC calls |
| `symbiotic_relay` | `retry_backoff` | 1s | Base backoff (linear: backoff × attempt) |
| `oz_relayer` | `base_url` | - | OZ Relayer HTTP endpoint |
| `oz_relayer` | `max_retries` | 3 | Retry attempts for HTTP calls |
| `oz_relayer` | `retry_backoff` | 1s | Base backoff (exponential: backoff × 2^attempt) |
| `oz_relayer` | `chain_relayers` | - | Per-chain relayer configuration |
| - | `destination_chains` | - | Chain IDs to submit proofs to |

Retryable errors: HTTP 429 (rate limit), 500-504 (server errors), network timeouts.

## Contract Addresses

After deployment, addresses are written to `data/deploy-data/`:

- `source_contracts.json` - DVN on source chain
- `dest_contracts.json` - DVN on destination chain
- `settlement_contract.json` - Settlement contract
- `relay_infra_contracts.json` - Symbiotic relay infrastructure

## Contributing

Contributions are welcome. Please open an issue to discuss significant changes before submitting a PR.

## License

MIT
