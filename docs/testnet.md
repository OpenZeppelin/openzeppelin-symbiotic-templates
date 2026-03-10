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

# Operator base key (required for testnet - default 1e18 addresses are
# compromised on public testnets)
OPERATOR_BASE_KEY=123456789000000000

# Relay timing overrides (recommended for testnet)
EPOCH_DURATION=60          # Driver epoch length in seconds (prod default: 28800 = 8h)
SLASHING_WINDOW=300        # Vault epoch / slashing window (prod default: 86400 = 1 day)
EPOCH_START_DELAY=600      # Delay before epoch 0 starts (prod default: 0)
```

| Variable | Default (production) | Testnet recommended | Purpose |
| --- | --- | --- | --- |
| `EPOCH_DURATION` | 28800 (8h) | 60 (1 min) | Driver epoch length — how often validator sets are captured |
| `SLASHING_WINDOW` | 86400 (1 day) | 300 (5 min) | Vault epoch duration — deposits activate after one epoch |
| `EPOCH_START_DELAY` | 0 | 600 (10 min) | Delays epoch 0 start so operators can register before any epochs exist |

> **Why these matter:** With production defaults, vault deposits take 24 hours to activate (one `SLASHING_WINDOW` epoch). Setting `SLASHING_WINDOW=300` makes deposits activate in 5 minutes. `EPOCH_START_DELAY` gives a window to register operators before epoch counting begins, preventing the "epoch gap" problem where sidecars encounter epochs with no BLS keys registered.

> **Important:** For local anvil mode, comment out or remove `SOURCE_RPC_URL`, `DEST_RPC_URL`, `PRIVATE_KEY`, and `OPERATOR_BASE_KEY`. The relay timing variables can also be removed — local mode uses the defaults which work with anvil's time manipulation.

### 2. Verify the testnet config

Review `config/root.config.testnet.json`:

```json
{
  "version": 1,
  "active_provider": "layerzero",
  "providers": {
    "layerzero": {
      "source_chain_id": 84532,
      "destination_chain_id": 11155111,
      "source_eid": 40245,
      "destination_eid": 40161
    }
  }
}
```

The chain IDs and EIDs must match real LayerZero V2 endpoint IDs.

### 3. Start the stack

```bash
make start ROOT_CONFIG_FILE=config/root.config.testnet.json
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
[5/7] Generate configs (monitor, relayer, operators)
[6/7] Preflight checks
[7/7] Start services
```

### 4. Run E2E test

```bash
make e2e ROOT_CONFIG_FILE=config/root.config.testnet.json
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

Local mode layers `docker-compose.local.yml` on top of `docker-compose.yml` to add anvil chains and override RPC URLs. Testnet uses only `docker-compose.yml`. The Makefile detects this automatically based on `source_chain_id` in the root config.

### Contract Deployment

| Phase | Local | Testnet |
|-------|-------|---------|
| Symbiotic Core | Deployed fresh | Loaded from `config/networks/symbiotic-core.json` |
| LayerZero endpoints | Mock contracts deployed | Pre-deployed V2 addresses from `config/networks/layerzero-endpoints.json` |
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
FORCE_RELAY_DEPLOY=1 make start ROOT_CONFIG_FILE=config/root.config.testnet.json
```

### Operator Registration

On local anvil, operators are registered inside `DeployRelayInfra.s.sol` using auto-impersonation. On testnet, this is split into two phases:

1. **Fund operators** - Deployer sends ETH + staking tokens to each operator address via `cast send`
2. **Register operators** - Each operator registers via `RegisterOperators.s.sol` (register in OperatorRegistry, opt-in to network/vault, deposit stake, register BLS keys)

### Operator Base Key

Operator private keys are derived deterministically: `OPERATOR_BASE_KEY + index`. The default `1e18` (1000000000000000000) produces addresses that are compromised on public testnets (contracts deployed at those addresses that drain ETH).

Set `OPERATOR_BASE_KEY` in `.env` to a different value for testnet. The value propagates to:

- Solidity scripts (`DeployRelayInfra.s.sol`, `RegisterOperators.s.sol`)
- Docker sidecar startup (`start-sidecar.sh`)
- Genesis generation (`generate-genesis.sh`)

### Epoch Syncing

Symbiotic relay sidecars sync epoch/validator set data from the Driver contract on the settlement chain (Sepolia). For a fresh deployment, there are only a few epochs to sync, so startup is fast.

If you redeploy on an existing Driver contract with many epochs, sync can take longer and may hit RPC rate limits. Consider:

- Using a paid RPC plan (3 sidecars polling concurrently)
- Reducing to 1 sidecar for initial testing

## Other Make Commands

All standard commands accept `ROOT_CONFIG_FILE`:

```bash
make send ROOT_CONFIG_FILE=config/root.config.testnet.json MSG="hello"
make watch ROOT_CONFIG_FILE=config/root.config.testnet.json
make status ROOT_CONFIG_FILE=config/root.config.testnet.json
make status-msg ROOT_CONFIG_FILE=config/root.config.testnet.json
make stop
make logs-operators
make logs-relays
```

## Switching Between Local and Testnet

To switch from testnet back to local:

1. Comment out testnet values in `.env` (`SOURCE_RPC_URL`, `DEST_RPC_URL`, `PRIVATE_KEY`, `OPERATOR_BASE_KEY`)
2. Run:
   ```bash
   make stop
   make clean
   make start   # uses config/root.config.json (local) by default
   ```

To switch from local to testnet:

1. Uncomment testnet values in `.env`
2. Run:
   ```bash
   make stop
   make clean
   make start ROOT_CONFIG_FILE=config/root.config.testnet.json
   ```

> **Note:** `make clean` is required when switching between local and testnet because `deploy-state.json` contains chain IDs that must match the root config.

## Adding New Testnet Chains

To deploy on different chains:

1. Create a new root config (e.g., `config/root.config.mychain.json`) with the correct chain IDs and LayerZero EIDs.

2. Add LayerZero V2 endpoint addresses to `config/networks/layerzero-endpoints.json`:
   ```json
   {
     "<chain_id>": {
       "endpoint": "0x...",
       "sendUln302": "0x...",
       "receiveUln302": "0x..."
     }
   }
   ```

3. Add Symbiotic Core addresses to `config/networks/symbiotic-core.json` (if deploying Settlement on a new chain):
   ```json
   {
     "<chain_id>": {
       "vaultFactory": "0x...",
       "delegatorFactory": "0x...",
       "slasherFactory": "0x...",
       "networkRegistry": "0x...",
       "networkMiddlewareService": "0x...",
       "operatorRegistry": "0x...",
       "operatorVaultOptInService": "0x...",
       "operatorNetworkOptInService": "0x...",
       "vaultConfigurator": "0x..."
     }
   }
   ```

4. Ensure your deployer has ETH on both chains.

5. Run `make start ROOT_CONFIG_FILE=config/root.config.mychain.json`.

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

Well-known derived private keys (like `1e18 + index`) have been compromised on public testnets. Set `OPERATOR_BASE_KEY` in `.env` to a different value and redeploy.

### Sidecar crashes with "failed to find key by keyTag" after fresh deploy

When deploying fresh relay infra (`FORCE_RELAY_DEPLOY=1`) on a testnet where relay infra was previously deployed, the shared Symbiotic Core OperatorRegistry may contain epochs referencing BLS keys from the **old** KeyRegistry. The sidecar tries to sync all historical epochs and fails on those stale references.

Workaround: Avoid fresh relay infra deploys when possible — the default relay infra reuse path handles this automatically. If you must deploy fresh, use a different `OPERATOR_BASE_KEY` to avoid conflicts with previously registered operators.

### Deploy state chain ID mismatch

```
ERROR: providers.layerzero.source_chain_id (84532) does not match deploy-state (31337)
```

Run `make clean` before switching between local and testnet to clear stale deploy data.
