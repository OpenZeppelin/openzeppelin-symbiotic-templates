# Testnet Deployment

Deploy and operate the LayerZero stack on the shared testnet environment:

- source: Base Sepolia (`84532`)
- destination: Sepolia (`11155111`)
- provider: `layerzero`

The current public testnet flow is xtask-based. There is no `make start ENV=testnet`.

## Runtime Model

For `ENV=testnet`, the user-facing flow is:

```bash
make validate ENV=testnet
make deploy ENV=testnet
make refresh-genesis ENV=testnet   # when validation says genesis is stale
make run-operators ENV=testnet
make send ENV=testnet MSG="hello"
make watch ENV=testnet
make e2e ENV=testnet
```

What these commands do:

- `validate`: read-only checks for config, chain reachability, deployment state, operator state, and relayer signer safety
- `deploy`: deploys the managed contracts for the environment and regenerates service config
- `refresh-genesis`: refreshes settlement genesis without redeploying contracts
- `run-operators`: starts the non-local operator-side services

## Environment Inputs

The environment definition lives in [config/environments/testnet.json](/Users/dylan/github/symbiotic-layerzero-template/config/environments/testnet.json).

That file currently owns:

- chain IDs and EIDs
- default RPC URLs
- LayerZero predeploys
- Symbiotic Core predeploys
- relay timing

xtask resolves RPC URLs from the environment JSON first. `SOURCE_RPC_URL` and `DEST_RPC_URL` in `.env` are only fallback overrides.

Required `.env` values for testnet:

```bash
PRIVATE_KEY=0x<deployer key>
KEYSTORE_PASSPHRASE=<keystore passphrase>

OPERATOR_1_PRIVATE_KEY=0x<operator 1 key>
OPERATOR_2_PRIVATE_KEY=0x<operator 2 key>
OPERATOR_3_PRIVATE_KEY=0x<operator 3 key>
```

Optional relayer bootstrap inputs:

```bash
RELAYER_1_PRIVATE_KEY=0x<relayer signer 1 key>
RELAYER_2_PRIVATE_KEY=0x<relayer signer 2 key>
RELAYER_3_PRIVATE_KEY=0x<relayer signer 3 key>
```

Those relayer private keys are setup-time inputs only. The steady-state runtime source is the OZ relayer keystore files under `config/oz-relayer/keys/`.

## Setup

Generate local operator keys and relayer keystores with:

```bash
make setup
```

For public testnets:

- do not use known local/dev keys
- make sure the deployer, operators, and relayer signers all have testnet ETH

## Typical Workflow

### 1. Validate first

```bash
make validate ENV=testnet
```

This catches the common setup issues before any broadcast:

- missing deployer key
- missing operator keys
- relayer signer keystore problems
- underfunded accounts
- stale genesis

### 2. Deploy managed contracts

```bash
make deploy ENV=testnet
```

This deploys the managed LayerZero-side stack for the current testnet environment and updates:

- [deployments/testnet.json](/Users/dylan/github/symbiotic-layerzero-template/deployments/testnet.json)
- `generated/testnet/`

### 3. Refresh genesis when needed

If validation reports stale genesis:

```bash
make refresh-genesis ENV=testnet
```

Use this instead of redeploying when the contracts are already in place and only the committed settlement header is stale.

### 4. Start operator services

```bash
make run-operators ENV=testnet
```

This starts the non-local service set from `docker-compose.yml`:

- operators
- OZ monitor
- OZ relayer
- Symbiotic relay sidecars

### 5. Send and verify messages

```bash
make send ENV=testnet MSG="hello"
make watch ENV=testnet
```

Or run the full loop:

```bash
make e2e ENV=testnet MSG="hello"
```

## How Testnet Differs From Local

| Area | Local | Testnet |
| --- | --- | --- |
| entrypoint | `make start` | `make deploy` + `make run-operators` |
| chains | local Anvil | Base Sepolia + Sepolia |
| LayerZero endpoints | local mocks | predeployed |
| Symbiotic Core | deployed fresh | predeployed on destination |
| genesis refresh | folded into local startup | explicit `make refresh-genesis ENV=testnet` when stale |
| compose files | `docker-compose.yml` + `docker-compose.local.yml` | `docker-compose.yml` only |

## Troubleshooting

### `validation failed: genesis stale`

Refresh it directly:

```bash
make refresh-genesis ENV=testnet
```

### `insufficient funds for gas`

Fund the deployer address derived from `PRIVATE_KEY`.

### Sidecars fail with RPC rate limits

Three sidecars syncing at once can overwhelm weak testnet RPC plans.

Options:

- use higher-throughput RPCs
- temporarily reduce the number of sidecars
- avoid repeated cold-start syncs when possible

### Public testnet keys look drained immediately

Do not use known local/dev keys on public testnets. Generate fresh operator keys and fresh relayer signer keystores with `make setup`.

### Fresh relay deploys on shared testnet

Fresh relay infra deploys on a shared testnet are the most fragile path. Prefer:

- reusing the existing environment when possible
- `make refresh-genesis ENV=testnet` for stale-genesis repair

If you intentionally provision a fresh relay network, treat it as a new deployment state and make sure you are not accidentally reusing compromised keys.
