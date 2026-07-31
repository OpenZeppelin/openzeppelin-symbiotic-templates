# OpenZeppelin Symbiotic Templates

Templates for building provider-specific cross-chain verification integrations with [Symbiotic](https://symbiotic.fi/) shared security.

> [!WARNING]
> **Release Candidate — NOT AUDITED. Not for production use.**
>
> The Symbiotic CCV (Chainlink CCIP) contracts and the off-chain operator in this repo
> have **not been audited by OpenZeppelin.** Provided as-is for evaluation and integration
> testing only — do **not** deploy to mainnet or secure real funds. A security audit is
> recommended before any production use. Use at your own risk.

## Providers

Only one provider is active per environment, configured in `config/environments/<env>.json`.

| Provider      | `activeProvider` value | Local                       | Testnet                             |
| ------------- | ---------------------- | --------------------------- | ----------------------------------- |
| LayerZero DVN | `layerzero`            | Supported                   | Supported (Base Sepolia -> Sepolia) |
| Symbiotic CCV | `chainlink_ccv`        | Supported (mock local path) | Supported (Base Sepolia -> Sepolia, `ENV=testnet-ccv`) |

## Quick Start (Local)

Each provider has a dedicated local environment: `ENV=local` (LayerZero, the default) or `ENV=local-ccv` (Symbiotic CCV — it carries the extra CCV signers and executor config the LayerZero environment doesn't have).

```bash
make install   # one-time: install contract dependencies (pnpm)
make use ENV=local-ccv   # optional: persist ENV for the CCV path instead of repeating it per command
make chains
make deploy
make start
make status
make e2e
```

`make use` writes the choice to a gitignored `.make-env`; an explicit `ENV=<name>` on any command still overrides it, and `make help` shows the active environment.

## Quick Start (Testnet)

Testnet supports both providers: `layerzero` (`ENV=testnet`) and `chainlink_ccv` (`ENV=testnet-ccv`). The commands below use the default `ENV=testnet` (LayerZero); for the CCV path substitute `ENV=testnet-ccv`.

The bundled LayerZero environments also enable the starter `ExampleOApp` by default, so `make e2e ENV=testnet` works after deploy unless you explicitly opt out.

```bash
cargo xtask --env testnet generate-signer --name deployer --name operator-1 --name operator-2 --name operator-3
cargo xtask --env testnet generate-signer --name signer-1 --name signer-2 --name signer-3
make validate ENV=testnet
make deploy ENV=testnet
make refresh-genesis ENV=testnet   # only if validation says genesis is stale
make start ENV=testnet
make e2e ENV=testnet
```

`config/environments/testnet.json` is the default source of testnet RPC URLs. `SOURCE_RPC_URL` and `DEST_RPC_URL` in `.env` are fallback overrides only.

For provider-only environments, set `layerzero.oapp.enabled` to `false` before deploy. Validation still passes, but the LayerZero demo commands are intentionally unavailable.

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
