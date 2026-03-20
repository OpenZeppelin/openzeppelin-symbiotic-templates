# Multi-Provider Config Schema Spec

Status: Draft  
Date: 2026-02-05

## 1. Purpose

Define a template-level configuration system that supports multiple providers (LayerZero DVN, Symbiotic CCV, future providers) without rewriting core scripts and operator wiring each time.

This spec is focused on configuration, generation, and deployment wiring. It is not the operator code refactor spec.

## 2. Captured Product Constraints

From stakeholder input:
1. One root config system should define all providers.
2. Providers should run as standalone deployment units in Docker.
3. For now, do not run two providers in the same operator instance.
4. Exactly one provider is enabled at a time in a running stack.
5. Reuse the same operator infrastructure pattern where possible:
- monitor listens
- relay signs
- relayer submits

## 3. Current System (Step-Back Analysis)

## 3.1 Current Root Config Pipeline

Today there is a root configuration pipeline, but it is DVN-first:
1. Inputs:
- `.env`
- `config/templates/*`
- `data/deploy-data/*.json`
2. Generator:
- `scripts/generate-configs.sh`
- `scripts/generate-addresses.sh`
3. Runtime outputs:
- `data/generated-config/operator-{1..3}/config.json`
- `data/generated-config/oz-monitor/*`
4. Runtime mount points:
- `docker-compose.yml` mounts generated config into containers

## 3.2 Current Coupling Inventory

Hard coupling exists at multiple layers:
1. Root deploy orchestration:
- `Makefile` hardcodes LayerZero + DVN deployment phases and key names.
2. Config generation:
- `scripts/generate-configs.sh` reads `.dvn` from deploy-data JSON and patches `layerzero` and `dvn_address` fields.
3. Operator config schema:
- `operator/src/config/mod.rs` has `provider` selector, but relayer mapping uses `dvn_address`.
4. Monitor templates:
- `config/templates/oz-monitor/monitors/layerzero_job_assigned.json`
- `config/templates/oz-monitor/triggers/webhook_layerzero.json`
5. Tooling/docs:
- `scripts/msg` and docs reference DVN-specific events and verification checks.

Conclusion: a root config system exists, but it is not provider-platform neutral yet.

## 4. Target Template Model

## 4.1 Root Config Ownership

Introduce one root config file as single source of truth:
1. Global environment and chain topology.
2. Enabled providers and provider-specific settings.
3. Per-provider operator-unit deployment shape.

Proposed path:
- `config/root.config.json`

## 4.2 Provider Unit Model

A provider unit is the deployment boundary:
1. `monitor` ingestion config for that provider.
2. `operator` process config for that provider.
3. `symbiotic-relay` sidecar(s) for that provider.
4. `oz-relayer` mapping used by that provider's submit flow.

Rule: one unit runs one provider implementation.

This enforces your constraint that two providers are not mixed in one operator instance.

## 4.3 Single-Active Provider Execution

Root config defines all providers but activates only one at runtime:
1. `active_provider = "layerzero" | "chainlink_ccv" | ...`
2. Non-active providers remain configured but not deployed.

Generation outputs become provider-scoped:
1. `data/generated-config/providers/layerzero/*`
2. `data/generated-config/providers/chainlink_ccv/*`

Service names become provider-scoped in compose:
1. `operator-layerzero-1`
2. `operator-chainlink-ccv-1`
3. Same naming strategy for monitor/relay sidecars where duplicated.

## 4.4 Config Schema Direction

Minimal root schema direction:

```json
{
  "version": 1,
  "active_provider": "layerzero",
  "global": {
    "operator_count": 3,
    "chains": {
      "source": { "chain_id": 31337, "rpc": "http://anvil:8545" },
      "destination": { "chain_id": 31338, "rpc": "http://anvil-settlement:8546" }
    }
  },
  "providers": {
    "layerzero": {
      "deployment_unit": "standalone",
      "mode": "live",
      "contracts": {},
      "monitor": {},
      "operator": {},
      "submission": {}
    },
    "chainlink_ccv": {
      "deployment_unit": "standalone",
      "mode": "symbiotic_mock",
      "contracts": {},
      "monitor": {},
      "operator": {},
      "submission": {},
      "devtools": {}
    }
  }
}
```

Notes:
1. `mode: symbiotic_mock` means the template runs the Symbiotic-only CCV path with local mock OnRamp/OffRamp contracts.
2. `contracts` may be populated by deploy scripts or manually pinned addresses.
3. Validation rule: `active_provider` must exist under `providers`.

## 4.5 Operator Config Neutralization

Operator config should be protocol-neutral where possible:
1. Keep `provider`.
2. Keep provider-specific blocks (`layerzero`, `chainlink_ccv`, etc).
3. Rename relayer chain mapping field:
- from `dvn_address`
- to `target_address`

Provider adapters decide how `target_address` is interpreted.

## 5. Generation Pipeline Changes

Replace DVN-specific patching with provider-aware generation:
1. Read root config and the single `active_provider`.
2. Read deploy artifacts per provider.
3. Emit active-provider runtime configs into `data/generated-config/active/...`.
4. Optionally emit provider-scoped outputs for debugging.

Key principle: `scripts/generate-configs.sh` becomes a dispatcher, not a LayerZero patcher.

## 6. Deployment/Compose Changes

Current single stack should evolve into provider-selective composition:
1. Base infra profile:
- chains, shared redis, shared oz-relayer
2. Provider profiles (exactly one selected by `active_provider`):
- `provider-layerzero`
- `provider-chainlink-ccv`
3. `make start` starts base infra + selected provider profile only.
4. `oz-monitor` is shared and receives provider-specific generated monitor config for the active provider.

## 7. Compatibility Strategy

No backward compatibility is required for this template iteration.
Migration should optimize for simplicity and remove legacy DVN-only paths early.

## 8. Risks

1. Over-sharing infrastructure across providers can create hidden coupling.
2. Under-sharing can create operational overhead and duplicate configs.
3. If generator and compose naming are not standardized now, future provider additions will regress into ad hoc scripts.

## 9. Delivery Plan (Minimum Now vs Cleaner Later)

## 9.1 Minimum Changes To Get Off The Ground (MVP)

1. Add `config/root.config.json` with `active_provider`.
2. Refactor `scripts/generate-configs.sh` to generate only active-provider runtime config.
3. Rename `dvn_address` -> `target_address` in operator config structs and JSON templates.
4. Keep one operator binary and one docker compose file; switch behavior by `active_provider`.
5. Add Symbiotic CCV provider blocks in config using Symbiotic mock-mode defaults.

Result: CCV can ship fast without a full platform rewrite.

## 9.2 Cleanup and Simplification When We Have More Time

1. Split generator into provider modules (`generate-configs/layerzero`, `generate-configs/chainlink_ccv`).
2. Convert compose into base + provider includes or cleaner profile composition.
3. Normalize all scripts/docs (`scripts/msg`, troubleshooting, API docs) to provider-aware commands.
4. Remove remaining DVN naming across storage keys/log labels/CLI output.
5. Add provider conformance test suite to keep future integrations simple.

## 10. Proposed Next Specs

1. Operator Refactor Spec:
- Provider adapters for ingress/signing/submission.
2. Deployment Spec:
- Compose profiles and service naming conventions.
3. Devtools Spec:
- Symbiotic-only CCV mock components and replacement interfaces.
4. Contracts Spec:
- CCV verifier contracts and resolver wiring.
5. Test Matrix Spec:
- per-provider smoke tests + multi-provider coexistence tests.

## 11. Open Decisions For Interview

MVP defaults are now locked:
1. Root config format: JSON (`config/root.config.json`).
2. `oz-monitor`: shared process.
3. `oz-relayer`: shared process.

Future reconsideration triggers:
1. If provider-specific monitor workloads conflict operationally, split monitor per provider.
2. If relayer policy isolation is required, split relayer per provider.
