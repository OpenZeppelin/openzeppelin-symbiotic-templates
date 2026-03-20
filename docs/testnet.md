# Testnet Deployment

Deploy and test the Symbiotic LayerZero DVN on real testnets (Base Sepolia → Sepolia).

## Overview

Testnet mode deploys the same stack as local development but against real chains:

| Aspect | Local (Anvil) | Testnet |
|--------|---------------|---------|
| Source chain | Anvil (31337) | Base Sepolia (84532) |
| Settlement/Dest chain | Anvil (31338) | Sepolia (11155111) |
| LayerZero endpoints | Mock (deployed locally) | Real LZ V2 (pre-deployed) |
| Symbiotic Core | Deployed locally | Pre-deployed on Sepolia |
| Anvil containers | Yes (2) | No |
| Docker compose | `docker-compose.yml` + `docker-compose.local.yml` | `docker-compose.yml` only |
| Epoch sync time | Instant (1 epoch) | Seconds (fresh deployment has few epochs) |
| RPC source | localhost | Alchemy / Infura / etc. |

## Prerequisites

Everything from the main README, plus:

- **Testnet ETH** on both Base Sepolia and Sepolia for the deployer address
- **RPC endpoints** for both chains (Alchemy, Infura, or similar)
- At least ~0.5 ETH on Sepolia (for deploying relay infrastructure + operator registration). Subsequent runs reuse relay infra and need much less.
- At least ~0.1 ETH on Base Sepolia (for deploying DVN + TestOApp)

## Step-by-Step Setup

### 1. Configure `.env`

Add or uncomment the testnet variables at the bottom of `.env`:

```bash
# Testnet config
SOURCE_RPC_URL=https://base-sepolia.g.alchemy.com/v2/<your-key>
DEST_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/<your-key>
PRIVATE_KEY=0x<deployer-private-key>

# Operator private keys (one per operator — run 'make setup' to generate)
OPERATOR_1_PRIVATE_KEY=0x<64 hex chars>
OPERATOR_2_PRIVATE_KEY=0x<64 hex chars>
OPERATOR_3_PRIVATE_KEY=0x<64 hex chars>

# Relay timing overrides (recommended for testnet)
EPOCH_DURATION=300          # Driver epoch length in seconds (prod default: 28800 = 8h)
SLASHING_WINDOW=300        # Vault epoch / slashing window (prod default: 86400 = 1 day)
EPOCH_START_DELAY=600      # Delay before epoch 0 starts (prod default: 0)
```

| Variable | Default (production) | Testnet recommended | Purpose |
| --- | --- | --- | --- |
| `EPOCH_DURATION` | 28800 (8h) | 300 (5 min) | Driver epoch length — how often validator sets are captured |
| `SLASHING_WINDOW` | 86400 (1 day) | 300 (5 min) | Vault epoch duration — deposits activate after one epoch |
| `EPOCH_START_DELAY` | 0 | 600 (10 min) | Delays epoch 0 start so operators can register before any epochs exist |

> **Why these matter:** With production defaults, vault deposits take 24 hours to activate (one `SLASHING_WINDOW` epoch). Setting `SLASHING_WINDOW=300` makes deposits activate in 5 minutes. `EPOCH_START_DELAY` gives a window to register operators before epoch counting begins, preventing the "epoch gap" problem where sidecars encounter epochs with no BLS keys registered.

> **Important:** For local anvil mode, comment out or remove `SOURCE_RPC_URL`, `DEST_RPC_URL`, `PRIVATE_KEY`, and the `OPERATOR_*_PRIVATE_KEY` variables. The relay timing variables can also be removed — local mode uses the defaults which work with anvil's time manipulation.

### 2. Verify the testnet config

Review `config/environments/testnet.json`. It contains chain IDs, EIDs, and pre-deployed contract addresses for both chains. The chain IDs and EIDs must match real LayerZero V2 endpoint IDs.

### 3. Start the stack

```bash
make start ENV=testnet
```

This runs the full deployment pipeline:

```
[1/7] Build contracts + operator image
[2/7] Verify external RPC connectivity
[3/7] Deploy contracts
      - Use pre-deployed LayerZero V2 endpoints
      - Deploy relay infrastructure (Driver, Settlement, KeyRegistry, etc.)
      - Fund and register 3 operators (BLS keys + staking)
      - Deploy DVN on both chains
      - Deploy TestOApp on both chains
      - Configure OApp ULN with DVN addresses
[4/7] Generate genesis validator set (commit to Settlement)
[5/7] Generate OZ configs (monitor, relayer)
[6/7] Preflight checks
[7/7] Start services
```

### 4. Run E2E test

```bash
make e2e ENV=testnet
```

Expected output:

```
E2E Test (LayerZero): send and verify destination target submission

Provider: layerzero
Sending message: "hello from e2e"
To EID: 40161

TX: 0x6677...
Block: 38637925

Watching LayerZero message (timeout: 120s)

[09:15:37] Destination target: verified on-chain (tx: 0x8e26...)

Message verified on destination chain
Dest TX: 0x8e26...
```

## How It Differs from Local

### Docker Compose

Local mode layers `docker-compose.local.yml` on top of `docker-compose.yml` to add anvil chains and override RPC URLs. Testnet uses only `docker-compose.yml`. The Makefile detects this automatically based on `.chains.source.chainId` in the environment JSON.

### Contract Deployment

| Phase | Local | Testnet |
|-------|-------|---------|
| Symbiotic Core | Deployed fresh | Pre-deployed addresses from `config/environments/testnet.json` |
| LayerZero endpoints | Mock contracts deployed | Pre-deployed V2 addresses from `config/environments/testnet.json` |
| Relay infra | Deployed fresh every time | **Reused** from `config/networks/relay-infra.json` if available on-chain; deployed fresh only on first run |
| Operators | Auto-registered in DeployRelayInfra | Registered separately (skipped when relay infra reused) |
| ULN config | Mock ULN defaults (applies globally) | Per-OApp ULN config (requires OApp addresses) |
| Genesis | Auto-committed | Auto-committed on first deploy; skipped when relay infra reused (already committed) |
| DVN + TestOApp | Deployed fresh | Always redeployed (cheap, may change during dev) |

### Relay Infra Reuse

On testnet, relay infrastructure (Driver, Settlement, KeyRegistry, Vault, etc.) is expensive to deploy and requires operator registration + genesis commitment. To avoid repeating this on every `make clean` + `make start` cycle, the start script:

1. Caches relay infra addresses in `config/networks/relay-infra.json` after first successful deployment
2. On subsequent starts, checks the cache and verifies contracts exist on-chain (`cast code`)
3. If verified, skips DeployRelayInfra, operator registration, and genesis (already committed)
4. DVN and TestOApp are still redeployed fresh each time

This means `make clean` + `make start` on testnet is fast — only DVN + TestOApp + config generation.

To force a fresh relay infra deployment (e.g., after changing operator keys or quorum):

```bash
FORCE_RELAY_DEPLOY=1 make start ENV=testnet
```

### Operator Registration

Operators are now registered inside `DeployRelayInfra.s.sol` for both local and testnet deployments.

- Local uses the default Anvil operator keys.
- Testnet funds each configured operator, registers it in the shared registries, deposits stake, registers BLS keys, and optionally tops up explicit relayer signers during the relay infra deploy step.

### Operator Private Keys

Each operator has its own EVM private key, set via environment variables:

```bash
OPERATOR_1_PRIVATE_KEY=0x<64 hex chars>
OPERATOR_2_PRIVATE_KEY=0x<64 hex chars>
OPERATOR_3_PRIVATE_KEY=0x<64 hex chars>
```

These keys are used as both EVM signing keys and BLS key seeds (the same scalar is used on the BN254 curve). The secondary BLS key for each operator is derived as `primary_key + 10000`.

Run `make setup` to generate random keys automatically. The keys propagate to:

- Solidity scripts (`DeployRelayInfra.s.sol`, `DeployLayerZeroStack.s.sol`)
- Docker sidecar startup (`start-sidecar.sh`)
- Genesis generation (`generate-genesis.sh`)
- OZ Relayer keystore + submitter derivation (`start-stack.sh`)

### Epoch Syncing

Symbiotic relay sidecars sync epoch/validator set data from the Driver contract on the settlement chain (Sepolia). For a fresh deployment, there are only a few epochs to sync, so startup is fast.

If you redeploy on an existing Driver contract with many epochs, sync can take longer and may hit RPC rate limits. Consider:

- Using a paid RPC plan (3 sidecars polling concurrently)
- Reducing to 1 sidecar for initial testing

## Other Make Commands

All standard commands accept `ENV`:

```bash
make send ENV=testnet MSG="hello"
make watch ENV=testnet
make status ENV=testnet
make status-msg ENV=testnet
make stop
make logs-operators
make logs-relays
```

## Switching Between Local and Testnet

To switch from testnet back to local:

1. Comment out testnet values in `.env` (`SOURCE_RPC_URL`, `DEST_RPC_URL`, `PRIVATE_KEY`, `OPERATOR_*_PRIVATE_KEY`)
2. Run:
   ```bash
   make stop
   make clean
   make start   # uses ENV=local by default
   ```

To switch from local to testnet:

1. Uncomment testnet values in `.env`
2. Run:
   ```bash
   make stop
   make clean
   make start ENV=testnet
   ```

> **Note:** `make clean` is required when switching between local and testnet to clear stale deployment data.

## Adding New Testnet Chains

To deploy on different chains:

1. Create a new environment JSON (e.g., `config/environments/mychain.json`) using `testnet.json` as a template. Set the correct chain IDs, EIDs, and pre-deployed addresses (LayerZero endpoints, Symbiotic Core) in the `predeploys` sections.

2. Ensure your deployer has ETH on both chains.

3. Run `make start ENV=mychain`.

## Troubleshooting

### "insufficient funds for gas"

The deployer or operator accounts don't have enough testnet ETH. Fund the deployer address shown in the error output.

### "no contract code at given address" during genesis

The `relay_utils` Docker image version doesn't match the deployed Driver ABI. Ensure `scripts/generate-genesis.sh` uses the same image tag as `docker-compose.yml`.

### Relay sidecars crashing with RPC errors

Free-tier RPC endpoints get rate-limited by 3 sidecars syncing concurrently. Options:
- Use a paid RPC plan
- Temporarily reduce to 1 sidecar (edit `docker-compose.yml`)
- Wait and restart — sidecar data is cached in `data/sidecar-*`

### Operator addresses have contract code on testnet

Well-known private keys have been compromised on public testnets. Run `make setup` to generate fresh random operator keys and redeploy with `FORCE_RELAY_DEPLOY=1`.

### Sidecar crashes with "failed to find key by keyTag" after fresh deploy

When deploying fresh relay infra (`FORCE_RELAY_DEPLOY=1`) on a testnet where relay infra was previously deployed, the shared Symbiotic Core OperatorRegistry may contain epochs referencing BLS keys from the **old** KeyRegistry. The sidecar tries to sync all historical epochs and fails on those stale references.

Workaround: Avoid fresh relay infra deploys when possible — the default relay infra reuse path handles this automatically. If you must deploy fresh, generate new operator keys (`make setup`) to avoid conflicts with previously registered operators.

### Chain ID mismatch after switching environments

Run `make clean` before switching between local and testnet to clear stale deployment data from the environment JSON.
