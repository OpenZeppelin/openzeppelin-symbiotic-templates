# OpenZeppelin Symbiotic Templates

Templates for building provider-specific cross-chain verification integrations with [Symbiotic](https://symbiotic.fi/) shared security.

## Providers

Only one provider is active per environment, configured in `config/environments/<env>.json`.

| Provider      | `activeProvider` value | Local                       | Testnet                             |
| ------------- | ---------------------- | --------------------------- | ----------------------------------- |
| LayerZero DVN | `layerzero`            | Supported                   | Supported (Base Sepolia -> Sepolia) |
| Symbiotic CCV | `chainlink_ccv`        | Supported (mock local path) | Not yet                             |

## Quick Start (Local)

```bash
# Select the provider in config/environments/local.json:
#   "activeProvider": "layerzero" | "chainlink_ccv"

make chains
make deploy
make start
make status
make e2e
```

## Quick Start (Testnet)

Testnet currently supports `layerzero` only.

```bash
cargo xtask generate-signer --name deployer --name operator-1 --name operator-2 --name operator-3
cargo xtask generate-signer --name signer-1 --name signer-2 --name signer-3
make validate ENV=testnet
make deploy ENV=testnet
make refresh-genesis ENV=testnet   # only if validation says genesis is stale
make start ENV=testnet
make e2e ENV=testnet
```

`config/environments/testnet.json` is the default source of testnet RPC URLs. `SOURCE_RPC_URL` and `DEST_RPC_URL` in `.env` are fallback overrides only.

## Core Commands

```bash
make chains
make deploy
make start
make e2e
make validate ENV=testnet
make deploy ENV=testnet
make start ENV=testnet
make dev-operator
make test
make help
```

## Repo Layout

```text
contracts/          Solidity contracts and Foundry tests
operator/           Rust operator service
config/             Environment JSON and monitor/relayer templates
scripts/            Deployment and workflow automation
deployments/        Canonical deployment addresses
generated/          Generated runtime config and message cache
```

## Generated State

- `deployments/<env>.json` stores canonical deployment addresses.
- `generated/<env>/` stores generated operator, monitor, relayer, and message-cache state.

## Docs

- [docs/index.mdx](docs/index.mdx) for the full docs index
- [docs/setup.mdx](docs/setup.mdx) for local setup
- [docs/deployment.mdx](docs/deployment.mdx) for testnet deployment
- [docs/layerzero.mdx](docs/layerzero.mdx) and [docs/chainlink-ccv.mdx](docs/chainlink-ccv.mdx) for provider details
- [docs/architecture.mdx](docs/architecture.mdx), [docs/security.mdx](docs/security.mdx), [docs/cli.mdx](docs/cli.mdx), and [docs/troubleshooting.mdx](docs/troubleshooting.mdx) for shared internals and operations

## License

- Solidity contracts (`contracts/`): [MIT](contracts/LICENSE)
- Operator, xtask, and all other code: AGPL-3.0
