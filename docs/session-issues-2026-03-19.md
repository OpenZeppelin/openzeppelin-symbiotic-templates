# Session Issues Analysis — Config Redesign & Testnet Deploy

Every issue we hit during the config redesign implementation and testnet testing, categorized by root cause. This is a post-mortem for improving the system.

## Category 1: Migration Completeness

Issues caused by incomplete migration — some code paths still read from old sources after the redesign.

### 1.1 generate-genesis.sh polled deleted file
`generate-genesis.sh` still polled for `data/deploy-data/relay_infra.json` after all other scripts migrated to the env JSON. **Root cause:** No systematic grep for `deploy-data` references after Phase 3.3. **Fix:** Now reads from `env_deployment()`.

### 1.2 Forge temp file outside fs_permissions
Symbiotic Core config written to `/tmp` via `mktemp`, but Forge's `fs_permissions` only allows `./` and `../config`. On macOS, `/tmp` maps to `/var/folders/...` which is outside scope. **Root cause:** The temp file was introduced when migrating from a config file that was inside the project. **Fix:** Write to `contracts/.tmp-symbiotic-core-config.json`.

### 1.3 Relay timing not centralized
Forge scripts read `EPOCH_DURATION`, `SLASHING_WINDOW`, `EPOCH_START_DELAY` from env vars, but `start-stack.sh` never passed them from the env JSON. The `.env` had hardcoded values that worked pre-redesign but were removed during cleanup. **Root cause:** Forge scripts are a separate config consumer that wasn't migrated. **Fix:** `start-stack.sh` now reads from `env_relay()` and passes to Forge.

## Category 2: .env Leakage (same bug, three instances)

Having testnet RPC URLs and deployer key in `.env` alongside local config causes silent failures when running local mode. The `.env` values override local defaults.

### 2.1 common.sh RPC defaults overridden
`SOURCE_RPC="${SOURCE_RPC_URL:-http://localhost:8545}"` — the `:-` syntax only uses the default when unset. `.env` sets `SOURCE_RPC_URL`, so local mode uses testnet RPCs. **Fix:** Hardcode: `SOURCE_RPC="http://localhost:8545"` for local.

### 2.2 msg script double-sourced .env
`.env` loaded at line 16 (before common.sh), then again in `load_runtime_context()` (after common.sh). Second load re-sets `PRIVATE_KEY` to testnet value, overriding common.sh's local hardcode. **Fix:** Removed second `.env` source.

### 2.3 Monitor sync used raw SOURCE_RPC_URL
`MONITOR_SOURCE_RPC="${MONITOR_SOURCE_RPC:-${SOURCE_RPC_URL:-...}}"` read from `.env` instead of using `SOURCE_RPC` (set by common.sh). Caused monitor to think block lag was 39M (Base Sepolia block number vs local anvil cursor). **Fix:** Use `${SOURCE_RPC}`.

### 2.4 Remaining exposure (from sub-agent analysis)
- `preflight-start.sh` reads `SOURCE_RPC_URL`/`DEST_RPC_URL` directly (safe because only runs on external networks, but fragile — relies on environment inheritance from parent)
- `configure-ccv-contracts.sh` and `deploy-ccv-contracts.sh` use `PRIVATE_KEY` fallback pattern (safe via Makefile passing, but inconsistent)

### Root cause
**`.env` is a shared file for all environments.** The "local mode ignores testnet vars" approach is fragile because every script that sources `.env` is a potential leak point. A better architecture would be environment-specific `.env` files (`.env.local`, `.env.testnet`) or no `.env` sourcing in scripts at all (let the Makefile handle it).

## Category 3: Rust Default vs Serde Defaults

### 3.1 SecurityConfig::default() gives wrong values
`from_environment()` used `..Default::default()` for SecurityConfig. Rust's `#[derive(Default)]` gives `Duration::default()` (0s) and `bool::default()` (false), not the serde defaults (`default_timestamp_window()` = 300s, `default_enable_debug_endpoints()` = true).

**Effects:**
- `timestamp_window: 0s` — all webhook timestamps "expired" — 401 on every webhook — operators never receive events
- `enable_debug_endpoints: false` — `make watch` can't query operators — silent timeout
- `webhook_secret: None` — would have returned 503 (masked by the timestamp issue)

**Fix:** Explicitly set all fields with custom defaults in `from_environment()`.

**Systemic risk:** Any struct with both `#[derive(Default)]` and `#[serde(default = "custom_fn")]` has this mismatch. Sub-agent confirmed all other structs in `from_environment()` are safe (all fields explicitly set), but the pattern is a landmine for future code.

**Prevention:** Consider removing `#[derive(Default)]` from config structs that have custom serde defaults, or adding a compile-time test that verifies `Default::default()` matches the expected serde defaults.

## Category 4: Docker Compose Orchestration

### 4.1 Sidecar env vars not propagated
`SIDECAR_DRIVER_ADDRESS` etc. only exported in `start-services.sh`, but `docker compose up` called from multiple places (Makefile targets, start-stack.sh resume path). Each invocation resolves `${SIDECAR_DRIVER_ADDRESS:-}` at call time — empty if not exported. **Fix:** `.env.deployments` file read by Docker Compose via `env_file`.

### 4.2 Orphan containers from different project name
Docker Compose derives project name from directory + compose files. Different `-f` flags = different project = `docker compose down` doesn't find old containers. **Fix:** `name: symbiotic-template` in docker-compose.yml.

### 4.3 make clean didn't clear env JSON deployments
`make clean` deleted `data/` but left deployment addresses in `local.json`. Next `make start` on fresh anvil tried to use stale addresses. **Fix:** `env_clear_deployments()` called in `make clean`.

### 4.4 rebuild-operators cascaded into sidecar starts
`docker compose up operator-1` triggers `depends_on: symbiotic-relay-1: condition: service_healthy`. Sidecars fail if `.env.deployments` doesn't exist. **Fix:** `--no-deps` flag on operator restart/rebuild targets.

## Category 5: Testnet Deployment Timing

### 5.1 EPOCH_START_DELAY=0 reverts on real chains
Forge script sets `epochDurationTimestamp = block.timestamp + epochStartDelay`. With delay=0, simulation-vs-execution timestamp drift causes `epochDurationTimestamp < block.timestamp` at execution — `EpochManager_InvalidEpochDurationTimestamp` revert. **Fix:** Defensive check in start-stack.sh; env JSON requires non-zero delay for external networks.

### 5.2 Genesis retry window shorter than stake activation
Genesis retries 60x5s = 5 min. But stake activation requires `EPOCH_START_DELAY + SLASHING_WINDOW` (currently 600+300 = 15 min). Script always times out on first deploy. **Fix needed:** Smart polling — read on-chain epoch params, calculate expected activation time, wait with countdown.

### 5.3 Zombie relay infra contracts
Previous failed deployment created contracts that exist on-chain but were never initialized (all storage = 0). `getCurrentEpoch()` underflows. Cache pointed to these zombies. **Fix needed:** Cache validation should check initialization state (e.g., epochDuration > 0), not just contract code existence.

### 5.4 Epoch params too slow for testnet iteration
`EPOCH_DURATION=300, SLASHING_WINDOW=300, EPOCH_START_DELAY=600` — 20 min from deploy to usable genesis. Painful for testing. **Fix needed:** Reduce to 120/120/180 (~7 min total).

## Category 6: Development Workflow

### 6.1 No E2E test after Phase 2
`from_environment()` was written with 22 unit tests that all passed, but never tested in Docker. The SecurityConfig bugs (Category 3) would have been caught by a single `make clean && make start && make e2e` run.

### 6.2 Operator image not rebuilt after code fix
Changed `from_environment()` in Rust source but ran `make start` which reused the cached Docker image. Had to explicitly `make rebuild-operators`. **Mitigation:** `make start` could hash the operator source and detect when a rebuild is needed.

### 6.3 OZ Relayer nonce tracking (pre-existing)
Relayer submits TXs but loses track on anvil restarts — gets stuck in "nonce too low" resubmission loop. Not caused by our changes. Affects local E2E reliability.

## Priority Order for Fixes

**High (blocking testnet):**
1. Smart genesis polling (5.2) — currently can't deploy testnet without manual intervention
2. Shorter epoch params (5.4) — reduces testnet deploy time from 20min to 7min
3. Cache validation for zombie contracts (5.3)

**Medium (reliability):**
4. Consistent .env handling (2.4) — remaining fragile patterns in preflight/CCV scripts
5. E2E test in CI (6.1) — prevent SecurityConfig-class bugs
6. Rust Default vs serde pattern guard (3.1) — prevent future mismatches

**Low (nice to have):**
7. Auto-detect operator rebuild needed (6.2)
8. Environment-specific .env files (2.root-cause)
