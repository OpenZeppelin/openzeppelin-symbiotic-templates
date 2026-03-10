# OpenZeppelin Symbiotic Templates

Templates for building cross-chain verification integrations with [Symbiotic](https://symbiotic.fi/) shared security.

## Providers

The repo is provider-centric and runs exactly one active provider per stack, configured in `config/root.config.json`.

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
#   edit config/root.config.json -> "active_provider": "layerzero" | "chainlink_ccv"

# Start stack (auto-bootstrap env + provider-aware deploy + configure + start)
make start

# Check service health
make status

# Run provider-aware end-to-end smoke (send + watch)
make e2e
```

## Quick Start (Testnet)

```bash
# 1. Configure .env with testnet values:
#    SOURCE_RPC_URL=https://base-sepolia.g.alchemy.com/v2/<key>
#    DEST_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/<key>
#    PRIVATE_KEY=0x<deployer-key-with-testnet-ETH-on-both-chains>
#    # Generate operator keys: make setup (writes OPERATOR_N_PRIVATE_KEY vars)

# 2. Set relay timing overrides for testnet (optional, recommended):
#    EPOCH_DURATION=300          # Driver epoch length in seconds (default: 28800 = 8h)
#    SLASHING_WINDOW=300        # Vault epoch / slashing window in seconds (default: 86400 = 1 day)
#    EPOCH_START_DELAY=600      # Delay before epoch 0 starts, allows operator registration (default: 0)

# 3. Start with testnet config
make start ROOT_CONFIG_FILE=config/root.config.testnet.json

# 4. Run E2E test
make e2e ROOT_CONFIG_FILE=config/root.config.testnet.json
```

> **Switching back to local:** Remove or comment out `SOURCE_RPC_URL`, `DEST_RPC_URL`, `PRIVATE_KEY`, `OPERATOR_*_PRIVATE_KEY`, and the relay timing variables from `.env`. Local anvil mode uses built-in defaults.

See [Testnet Deployment](docs/testnet.md) for detailed setup guide.

## Common Commands

```
make setup              Generate .env with operator keys
make install            Install dependencies (contracts npm packages)
make start              Smart start (provider-aware deploy + monitor sync wait)
make stop               Stop all containers (preserve state)
make clean              Full reset (stop + remove volumes + markers)

make restart-operators  Rebuild and restart all 3 operators
make restart-monitor    Restart oz-monitor (config reload)
make restart-relayer    Restart oz-relayer
make restart-relays     Restart symbiotic-relay-1/2/3

make send MSG="hello"   Provider-specific test message send
make watch              Watch latest message (requires prior send or --guid/--tx)
make status-msg         Quick operator status snapshot
make e2e                send + watch

make dev-operator       Run operator-1 locally (cargo run)
make rebuild-operators  Docker rebuild + restart all operators
make shell              Interactive shell with addresses loaded

make test               Run unit tests (forge + cargo)
make test-contracts     Run contract tests only

make configure          Regenerate configs from templates
make addresses          Generate addresses.env from deploy data

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
├── config/             # Root config + templates
│   ├── root.config.json
│   └── templates/
├── scripts/            # Automation scripts
├── data/
│   ├── generated-config/  # Generated runtime configs (gitignored)
│   └── deploy-data/       # Deployment artifacts
└── docker-compose.yml
```

## Contract Address Artifacts

After deploy/configure, canonical runtime artifacts are written under `data/deploy-data/`:

- `deploy-state.json` - Provider deployment state (both providers under `.providers.*`)
- `relay_infra.json` - Destination relay infra artifacts (includes settlement + registries)
- `addresses.env` - Shell-sourceable address exports derived from active provider + deploy state

## Docs

- [Architecture](docs/architecture.md) - System diagram, message flow, BLS signing
- [Testnet Deployment](docs/testnet.md) - Base Sepolia → Sepolia deployment guide
- [Operator Guide](docs/operator-guide.md) - Operator internals, modules, extending
- [Configuration](docs/configuration.md) - Environment variables, operator config, webhooks, retry settings
- [API Reference](docs/api-reference.md) - HTTP endpoints for webhooks, debugging, and proofs
- [CLI Reference](docs/cli-reference.md) - `scripts/msg` tool commands and options
- [Manual Testing](docs/testing/manual-testing.md) - Step-by-step testing with underlying commands
- [Security](docs/security.md) - Trust model, access control, invariants
- [Troubleshooting](docs/troubleshooting.md) - Common issues, debugging, log analysis

## License

MIT
