# OpenZeppelin Symbiotic Templates

Templates for building cross-chain verification integrations with [Symbiotic](https://symbiotic.fi/) shared security.

## Providers

The repo is provider-centric and runs exactly one active provider per stack, configured in `config/root.config.json`.

| Provider | `active_provider` value | Local status |
| --- | --- | --- |
| LayerZero DVN | `layerzero` | Supported |
| Symbiotic CCV (Chainlink CCIP-compatible verifier path) | `chainlink_ccv` | Supported (Symbiotic-only mock path) |

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

## Quick Start

```bash
# Optional: regenerate local .env + keys
make setup

# Select provider:
#   edit config/root.config.json -> "active_provider": "layerzero" | "chainlink_ccv"

# Start stack (auto-bootstrap env + provider-aware deploy + configure + start)
# Note: startup now waits for oz-monitor to be near chain head before returning.
make start

# Check service health
make status

# Run provider-aware end-to-end smoke (send + watch)
make e2e
```

## Common Commands

```bash
make setup              # Optional: regenerate .env + operator keys
make start              # Smart start (provider-aware deploy + monitor sync wait)
make stop               # Stop all (preserve state)
make clean              # Full reset (removes volumes + deploy markers)

make send MSG="hello"   # Provider-specific test message send
make watch              # Watch latest message (requires prior send or --guid/--tx)
make status-msg         # Quick operator status snapshot
make e2e                # send + watch

make configure          # Regenerate configs from templates
make addresses          # Regenerate data/deploy-data/addresses.env

make logs-operators     # Follow operator logs
make logs-monitor       # Follow OZ monitor logs
make logs-relayer       # Follow OZ relayer logs
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

After deploy/configure, addresses are written under `data/deploy-data/`:

- `source_contracts.json` - LayerZero source artifacts
- `dest_contracts.json` - LayerZero destination artifacts
- `ccv_source_contracts.json` - SymbioticCCV source artifacts
- `ccv_dest_contracts.json` - SymbioticCCV destination artifacts
- `relay_infra.json` - Symbiotic relay infra artifacts (includes Settlement)
- `addresses.env` - Shell-sourceable address exports

## Docs

- `docs/configuration.md`
- `docs/architecture.md`
- `docs/api-reference.md`
- `docs/troubleshooting.md`
- `docs/devnet-issues.md`

## License

MIT
