# OpenZeppelin Symbiotic Templates

Templates for building cross-chain verification integrations with [Symbiotic](https://symbiotic.fi/) shared security.

## Providers

The repo is provider-centric and runs exactly one active provider per stack, configured in the environment JSON (`config/environments/{local,testnet,mainnet}.json`).

| Provider | `active_provider` value | Local | Testnet |
| --- | --- | --- | --- |
| LayerZero DVN | `layerzero` | Supported | Supported (Base Sepolia → Sepolia) |
| Symbiotic CCV (Chainlink CCIP-compatible verifier path) | `chainlink_ccv` | Supported (Symbiotic-only mock path) | Not yet |

For the CCV provider, local dev uses:
1. Source-chain `CCIPMessageSent` events emitted on-chain.
2. Symbiotic operators + relay sidecars for BLS signing.
3. OZ relayer submission to destination OffRamp-compatible mock.
4. Destination verifier execution via `SymbioticCCV.verifyMessage(...)`.

No Chainlink auxiliary devenv stack (`aggregator/indexer/verifier/executor`) is required for this template path.

## Prerequisites

- Docker and Docker Compose v2+
- [Foundry](https://book.getfoundry.sh/getting-started/installation) (`forge`, `cast`, `anvil`)
- [Rust/Cargo](https://rustup.rs/) (for `make dev-operator`)
- `jq`

## Quick Start (Local)

```bash
# Optional: regenerate local .env + keys
make setup

# Select provider:
#   edit config/environments/local.json -> "activeProvider": "layerzero" | "chainlink_ccv"

# Start stack (auto-bootstrap env + provider-aware deploy + start)
make start

# Check service health
make status

# Run provider-aware end-to-end smoke (send + watch)
make e2e
```

## Quick Start (Testnet)

```bash
# 1. Generate operator keys and relayer keystores if needed
make setup

# 2. Configure .env with at least:
#    PRIVATE_KEY=0x<deployer-key-with-testnet-ETH>
#    KEYSTORE_PASSPHRASE=<keystore passphrase>
#    OPERATOR_1_PRIVATE_KEY=0x...
#    OPERATOR_2_PRIVATE_KEY=0x...
#    OPERATOR_3_PRIVATE_KEY=0x...

# 3. Validate the shared testnet environment first
make validate ENV=testnet

# 4. Deploy managed contracts and configs
make deploy ENV=testnet

# 5. Refresh genesis if validation reports it stale
make refresh-genesis ENV=testnet

# 6. Start operator-side services
make run-operators ENV=testnet

# 7. Run E2E test
make e2e ENV=testnet
```

> **RPC resolution:** `config/environments/testnet.json` is the default source of testnet RPC URLs. `SOURCE_RPC_URL` and `DEST_RPC_URL` in `.env` are only fallback overrides.

See [Testnet Deployment](docs/testnet.md) for detailed setup guide.

## Common Commands

```
make setup              Generate .env with operator keys
make install            Install dependencies (contracts npm packages)
make start              Start the full local stack
make deploy             Deploy contracts and generate service config
make validate           Run read-only validation checks
make run-operators      Start non-local operator services
make stop               Stop all containers (preserve state)
make clean              Full reset (stop + remove volumes + markers)

make restart-operators  Rebuild and restart all 3 operators
make restart-monitor    Restart oz-monitor (config reload)
make restart-relayer    Restart oz-relayer
make restart-relays     Restart symbiotic-relay-1/2/3

make send MSG="hello"   Provider-specific test message send
make watch              Watch latest message (requires prior send or --id/--tx)
make e2e                send + watch

make dev-operator       Run operator-1 locally (cargo run)
make rebuild-operators  Docker rebuild + restart all operators
make shell              Interactive shell with addresses loaded

make test               Run unit tests (forge + cargo)
make test-contracts     Run contract tests only

make logs-operators     Follow all 3 operator logs
make logs-operator-N    Follow operator-N logs (N=1,2,3)
make logs-monitor       Follow oz-monitor logs
make logs-relayer       Follow oz-relayer logs
make logs-relays        Follow symbiotic-relay-1/2/3 logs

make status             Show running containers and health
make help               Show all available commands
```

## Project Structure

```text
├── contracts/          # Solidity contracts (provider contracts + shared mocks)
├── operator/           # Rust operator service
├── config/
│   ├── environments/   # Per-network config (local.json, testnet.json, mainnet.json)
│   └── templates/      # OZ Monitor/Relayer templates
├── scripts/            # Automation scripts
├── data/               # Runtime data (gitignored)
└── docker-compose.yml
```

## Generated State

After deploy/start, committed and generated runtime state lives in:

- `deployments/<env>.json` - canonical deployment addresses for the selected environment
- `generated/<env>/` - generated service config and message cache

## Docs

- [Architecture](docs/architecture.md) - System diagram, message flow, BLS signing
- [Testnet Deployment](docs/testnet.md) - Base Sepolia → Sepolia deployment guide
- [Operator Guide](docs/operator-guide.md) - Operator internals, modules, extending
- [Configuration](docs/configuration.md) - Environment variables, operator config, webhooks, retry settings
- [API Reference](docs/api-reference.md) - HTTP endpoints for webhooks, debugging, and proofs
- [CLI Reference](docs/cli-reference.md) - `make send/watch/e2e` and `cargo xtask msg`
- [Manual Testing](docs/testing/manual-testing.md) - Step-by-step testing with underlying commands
- [Security](docs/security.md) - Trust model, access control, invariants
- [Troubleshooting](docs/troubleshooting.md) - Common issues, debugging, log analysis

## License

MIT
