# Configuration System Redesign

> Research and architecture for a unified, simplified configuration system.
> Date: 2026-03-13

## Problem Statement

The current configuration system has 5+ input config files, 5+ generated files, and 7+ shell scripts with `is_local()` branching everywhere. Setup is environment-unaware — `make setup` writes local defaults, testnet requires hand-editing `.env` afterward. Secrets, network config, timing, and operational config are all mixed in one flat `.env` (~50 vars).

## Research Summary

### Ecosystem Patterns (2025-2026)

| Project | Pattern |
|---------|---------|
| EigenDA, AltLayer MACH | Directory-per-network (`ethereum/`, `holesky/`) |
| Obol Charon | `.env.sample.mainnet`, `.env.sample.hoodi` → copy to `.env` |
| Gasp AVS | `.env.mainnet`, `.env.testnet` → copy to `.env` |
| SSV Network | Single `.env` + `NETWORK=mainnet\|hoodi` flag |
| Chainlink | `config.toml` + `secrets.toml` (structured, not env vars) |
| Hyperlane | Layered JSON config + `HYP_` env var overrides |
| Cosmos/Hermes | Single config with `[[chains]]` array, keys managed by CLI |

**Key findings:**
- Most production teams keep it simple — per-network templates or directory-per-network
- Structured config (JSON/TOML) beats flat `.env` for anything beyond secrets
- Keys managed by CLI, referenced by name, not stored in config files
- The "derive at runtime" pattern is universally preferred over pre-generation
- RPC URLs are secrets (contain API keys) — they belong in `.env`, not config
- Chainlink's migration from env vars to structured config is the canonical lesson

### Foundry Capabilities (v1.0-v1.4+)

- `foundry.toml` profiles with `[rpc_endpoints]` and `${ENV_VAR}` interpolation
- `script/input/<chainId>/params.json` — official convention for chain-specific deploy params
- `broadcast/ScriptName.s.sol/<chainId>/run-latest.json` — automatic deployment registry
- `vm.getDeployment("ContractName", chainId)` — built-in cheatcode (v1.0+) to read from broadcast/
- `vm.writeJson` — Forge scripts can write JSON output files
- `StdConfig` (v1.4+) — chain-ID-keyed TOML config with `_loadConfigAndForks()`
- `cast wallet import` + `--account` flag — encrypted keystores, never raw private keys

### What Paradigm Would Do

- Use `vm.getDeployment` and broadcast/ as the deployment registry
- Use `cast wallet import` for key management
- Keep deploy scripts self-contained in Solidity
- Use foundry.toml profiles + [rpc_endpoints] for RPC URLs
- Use structured config files, not flat `.env` for non-secret configuration

---

## Current System Analysis

### State Machine

Only 4 real states exist:

```
UNCONFIGURED ──make setup──> CONFIGURED ──make deploy──> DEPLOYED ──make up──> RUNNING
     ^                           |              ^                      |
     └────── make distclean ─────┴── make clean ┴──── make stop ──────┘
```

Detection via three file-existence checks:
- `.env` + keystores exist → CONFIGURED
- Expected broadcast/ files exist → DEPLOYED
- Docker containers running → RUNNING

### Current Config Files (the mess)

**Input files (5+):**
1. `config/root.config.json` — chain IDs, EIDs, active provider for local
2. `config/root.config.testnet.json` — same schema for testnet
3. `config/networks/layerzero-endpoints.json` — pre-deployed LZ endpoint addresses
4. `config/networks/symbiotic-core.json` — pre-deployed Symbiotic Core addresses
5. `.env` — ~50 vars mixing secrets + config + timing + everything
6. `config/templates/operator/config.json` — template with PLACEHOLDER values

**Generated files (5+):**
7. `deploy-state.json` — consolidated deployed addresses
8. `data/generated-config/operator-{1,2,3}/config.json` — per-operator runtime config
9. `data/generated-config/oz-monitor/` — monitor job configs + network definitions
10. `data/generated-config/oz-relayer/config.json` — relayer config
11. `addresses.env` — shell-sourceable deployed addresses

### Data Flow

| Category | Origin | Storage | Consumers |
|----------|--------|---------|-----------|
| Chain Topology | root.config.json | in-memory | Makefile, Forge, operator config |
| RPC URLs | .env | docker-compose env | Forge, cast, sidecars, monitors |
| Private Keys | setup.sh → .env | OZ relayer keystore | Forge, cast, sidecars |
| Timing | .env | Forge env vars | Relay infra deployment |
| Deployed Addresses | Forge → deploy-state.json | data/deploy-data/ | generate-configs, preflight |
| Service Config | .env, templates | docker-compose env | oz-monitor, oz-relayer, operators |
| Operator Config | Template + deploy-state | generated-config/ | Operator containers |

### The Sidecar Pattern (what works well)

The sidecar (`start-sidecar.sh`) derives ALL its config at container startup from just two inputs:
- `OPERATOR_N_PRIVATE_KEY` (env var from docker-compose)
- `deploy-state.json` + `relay_infra.json` (mounted volumes)

It reads chain IDs from deploy-state, derives BLS keys from the operator key (primary + secondary via +10000 offset), reads the driver address from relay_infra.json. Zero config generation needed. This is the pattern we want everywhere.

### Docker Compose Env Vars Actually Needed

Secrets that MUST be in `.env`:
- `WEBHOOK_SECRET` (oz-monitor, operators)
- `OZ_RELAYER_API_KEY` (oz-relayer, operators)
- `OZ_RELAYER_WEBHOOK_SECRET` (oz-relayer)
- `KEYSTORE_PASSPHRASE` (oz-relayer)
- `OPERATOR_{1,2,3}_PRIVATE_KEY` (sidecars)
- `SOURCE_RPC_URL`, `DEST_RPC_URL` (sidecars, external networks)
- `PRIVATE_KEY` (deployer, used by shell scripts only)

Everything else is either hardcoded in docker-compose.yml or comes from mounted config files.

---

## Target Architecture: 2 Input Files, Zero Generation

### File Structure

```
config/environments/
├── local.json                              # committed — full environment definition
├── testnet.json                            # committed — full environment definition
└── mainnet.json                            # committed — future

.env                                        # gitignored — 10 secrets only
.env.example                                # committed — documents required vars

contracts/broadcast/                        # Foundry-managed deployment registry
├── DeployDVN.s.sol/84532/run-latest.json   # (replaces deploy-state.json)
├── DeployDVN.s.sol/11155111/run-latest.json
├── DeployRelayInfra.s.sol/11155111/...
└── ...

docker/entrypoints/
├── oz-monitor.sh                           # renders OZ config at container boot
└── oz-relayer.sh                           # renders OZ config at container boot

state/<env>/                                # runtime data (Docker volumes)
state/keystores/signer-{1,2,3}.json         # generated once by make setup
```

### What Gets Deleted

| Current | Fate |
|---------|------|
| `config/root.config.json` | Absorbed into `config/environments/local.json` |
| `config/root.config.testnet.json` | Absorbed into `config/environments/testnet.json` |
| `config/networks/layerzero-endpoints.json` | Absorbed into `environments/*.json` predeploys |
| `config/networks/symbiotic-core.json` | Absorbed into `environments/*.json` predeploys |
| `config/networks/relay-infra.json` | broadcast/ replaces this |
| `config/templates/operator/config.json` | Operator derives at runtime |
| `data/deploy-data/deploy-state.json` | broadcast/ replaces this |
| `data/deploy-data/addresses.env` | Deleted |
| `data/generated-config/**` | Deleted entirely |
| `scripts/generate-configs.sh` | Deleted |
| `scripts/generate-addresses.sh` | Deleted |
| `scripts/update-deploy-state.sh` | Deleted |
| 40+ `.env` vars (non-secret) | Moved to environment JSON |

### Environment JSON Schema

`config/environments/testnet.json`:

```json
{
  "version": 1,
  "name": "testnet",
  "chains": {
    "source": {
      "name": "base-sepolia",
      "chainId": 84532,
      "eid": 40245,
      "confirmations": 3,
      "blockTimeMs": 2000,
      "predeploys": {
        "layerzero": {
          "endpoint": "0x6EDCE65403992e310A62460808c4b910D972f10f",
          "sendUln302": "0xC1868e054425D378095A003EcbA3823a5D0135C9"
        }
      }
    },
    "destination": {
      "name": "sepolia",
      "chainId": 11155111,
      "eid": 40161,
      "confirmations": 3,
      "blockTimeMs": 12000,
      "predeploys": {
        "layerzero": {
          "endpoint": "0x6EDCE65403992e310A62460808c4b910D972f10f",
          "receiveUln302": "0xdAf00F5eE2158dD58E0d3857851c432E34A3A851"
        },
        "symbioticCore": {
          "vaultFactory": "0x407A039D94948484D356eFB765b3c74382A050B4",
          "delegatorFactory": "0x890CA3f95E0f40a79885B7400926544B2214B03f",
          "slasherFactory": "0xbf34bf75bb779c383267736c53a4ae86ac7bB299",
          "networkRegistry": "0x7d03b7343BF8d5cEC7C0C27ecE084a20113D15C9",
          "networkMiddlewareService": "0x62a1ddfD86b4c1636759d9286D3A0EC722D086e3",
          "operatorRegistry": "0x6F75a4ffF97326A00e52662d82EA4FdE86a2C548",
          "operatorVaultOptInService": "0x95CC0a052ae33941877c9619835A233D21D57351",
          "operatorNetworkOptInService": "0x58973d16FFA900D11fC22e5e2B6840d9f7e13401",
          "vaultConfigurator": "0xD2191FE92987171691d552C219b8caEf186eb9cA"
        }
      }
    }
  },
  "relay": {
    "epochDurationSeconds": 300,
    "slashingWindowSeconds": 300,
    "epochStartDelaySeconds": 600
  },
  "operator": {
    "logLevel": "info",
    "eventPollInterval": "30s",
    "signJobInterval": "2s",
    "signWorkerCount": 2,
    "minBatchSize": 1
  },
  "ozMonitor": {
    "cronSchedule": "*/15 * * * * *",
    "maxPastBlocks": 50
  },
  "ozRelayer": {
    "requiredConfirmations": 3,
    "defaultSpeed": "fast",
    "minBalanceWei": "10000000000000000"
  }
}
```

`config/environments/local.json` is the same shape with `31337/31338`, short timings, `confirmations: 1`, and empty `predeploys`.

### .env (Secrets Only)

```bash
PRIVATE_KEY=0x...
OPERATOR_1_PRIVATE_KEY=0x...
OPERATOR_2_PRIVATE_KEY=0x...
OPERATOR_3_PRIVATE_KEY=0x...
SOURCE_RPC_URL=https://...
DEST_RPC_URL=https://...
WEBHOOK_SECRET=...
OZ_RELAYER_API_KEY=...
OZ_RELAYER_WEBHOOK_SECRET=...
KEYSTORE_PASSPHRASE=...
```

10 variables. No timing, no chain IDs, no addresses, no SIDECAR_N_SECRET_KEYS.

### Foundry broadcast/ as Deployment Registry

Instead of our custom pipeline (forge output → individual JSONs → `update-deploy-state.sh` → `deploy-state.json`), use Foundry's built-in deployment registry:

- **Forge scripts** use `vm.getDeployment("DVN", 84532)` to read previous deployments
- **Shell scripts** use `jq` over `contracts/broadcast/DeployDVN.s.sol/84532/run-latest.json`
- **Rust operator** reads broadcast JSONs to find deployed addresses
- **DEPLOYED state** = existence of expected broadcast files for the target chain IDs

DEPLOYED for testnet means these files exist:
```
contracts/broadcast/DeployRelayInfra.s.sol/11155111/run-latest.json
contracts/broadcast/DeployDVN.s.sol/84532/run-latest.json
contracts/broadcast/DeployDVN.s.sol/11155111/run-latest.json
contracts/broadcast/examples/DeployTestOApp.s.sol/84532/run-latest.json
contracts/broadcast/examples/DeployTestOApp.s.sol/11155111/run-latest.json
contracts/broadcast/RegisterOperators.s.sol/11155111/run-latest.json
```

Testnet/mainnet broadcast artifacts get committed to git (shared deployment state). Local broadcast stays disposable.

### Runtime Derivation (How Each Service Gets Config)

| Service | Inputs | How |
|---------|--------|-----|
| **Operator** (Rust) | env JSON + broadcast/ + `OPERATOR_INDEX` + secrets env | Reads at startup, builds config in memory |
| **Sidecar** | `OPERATOR_N_PRIVATE_KEY` + broadcast/ + env JSON | Derives BLS keys, reads chain IDs from broadcast |
| **OZ Monitor** | env JSON + broadcast/ + `SOURCE_RPC_URL` + `WEBHOOK_SECRET` | Entrypoint script renders config at boot, then `exec` |
| **OZ Relayer** | env JSON + keystores + `DEST_RPC_URL` + secrets | Entrypoint script renders config at boot, then `exec` |

Docker Compose mounts for every service:
- `./config/environments/${ENV}.json:/config/environment.json:ro`
- `./contracts/broadcast/:/config/broadcast/:ro`

### Makefile Interface

```makefile
ENV ?= local
CONFIG := config/environments/$(ENV).json

setup     # ensure .env exists, generate missing operator keys, build relayer keystores
doctor    # validate .env, config file, RPC reachability, chain IDs, required artifacts
deploy    # idempotent deploy/configure using Foundry; writes only broadcast artifacts
up        # docker compose up; services self-derive config at boot
start     # setup + doctor + deploy + up
stop      # docker compose down
clean     # stop + delete state/$(ENV)/ + local broadcast/
redeploy  # delete env-specific broadcast artifacts, then deploy
status    # docker compose ps
logs      # docker compose logs -f $(SERVICE)
e2e       # send + watch
```

Environment switching:
- `make start` → ENV=local (default)
- `make start ENV=testnet`
- `make start ENV=mainnet`
- `ENV` picks `config/environments/<env>.json`
- `ENV=local` also adds `docker-compose.local.yml`
- Runtime data namespaced under `state/<env>/`

### Developer Experience

**Local (zero config):**
```bash
git clone ... && cd ...
make start                    # everything just works
```

**Testnet:**
```bash
cp .env.example .env          # fill 10 secret values
make start ENV=testnet        # deploys + starts
```

---

## Implementation Requirements

### Changes Required

1. **Rust operator binary** — read `environment.json` + broadcast/ instead of generated `config.json`. Build config in memory at startup. (Moderate Rust work)

2. **Two entrypoint scripts** — `docker/entrypoints/oz-monitor.sh` and `oz-relayer.sh` that render the upstream OZ config format from env JSON + broadcast/ at container boot, then `exec` the real process. (~30 lines each)

3. **Broadcast resolver** — one shared function (shell + Rust) that extracts addresses from broadcast JSONs by contract name + chain ID.

4. **Sidecar update** — read broadcast/ instead of deploy-state.json for chain IDs and driver address.

5. **Forge script updates** — use `vm.getDeployment()` for cross-script address resolution instead of reading custom JSON files.

6. **Create environment JSONs** — merge root.config + networks/layerzero-endpoints + networks/symbiotic-core into `config/environments/{local,testnet}.json`.

7. **Simplify Makefile** — replace `ROOT_CONFIG_FILE` with `ENV`, remove `configure`/`addresses` targets.

8. **Docker Compose updates** — mount env JSON + broadcast/ instead of generated-config/.

### What Stays Unchanged

- Docker Compose overlay pattern (`docker-compose.yml` + `docker-compose.local.yml`)
- Forge deployment scripts (just change how they read/write config)
- Sidecar BLS key derivation logic
- OZ service images (upstream, we just change entrypoints)
