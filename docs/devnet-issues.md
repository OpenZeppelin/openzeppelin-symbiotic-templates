# Symbiotic CCV Devnet Issues Tracker

Status: Active  
Last Updated: 2026-02-06

## Goal

Track remaining work for the **Symbiotic-only CCV** devnet path:
- source `CCIPMessageSent` ingress
- operator + symbiotic relay signing
- relayer submission to destination `OffRamp.execute`
- destination verifier execution through `SymbioticCCV.verifyMessage`

No Chainlink CCV auxiliary devenv components are in scope for this template.

## Current State

1. `make send` uses a real source-chain tx on mock OnRamp and emits `CCIPMessageSent`.
2. `oz-monitor` ingests on-chain logs and forwards webhooks to operators.
3. Operators sign and relayer submits destination execute payload.
4. Destination mock OffRamp executes verifier checks by calling `SymbioticCCV.verifyMessage`.
5. `make watch` now waits for destination `MessageExecuted(messageId)` confirmation (on-chain), not just relayer submission state.

## Infra Requirements Clarification

For this template's Symbiotic CCV path, required local services are:
1. source + destination chains (`anvil`, `anvil-settlement`)
2. `oz-monitor`
3. `operator-1..3`
4. `symbiotic-relay-1..3`
5. `oz-relayer`
6. `redis`

Not required for this path:
1. Chainlink CCV auxiliary devenv services (`aggregator`, `indexer`, `verifier`, `executor`)

## Open Issues

## CCV-001: Epoch Staleness Can Fail Destination Verification

- Priority: P0
- Problem: destination execute can revert with `EpochTooStale()` from `SymbioticCCV`.
- Context: this appears when the settlement capture timestamp used in verifier results is outside `MAX_EPOCH_VALIDITY`.
- Target: keep local settlement epochs fresh or relax stale-check behavior for devnet profile.
- Acceptance:
1. `make send && make watch` succeeds consistently in normal dev sessions.
2. no frequent `Submission state: Failed` caused by stale epoch.

## CCV-002: Deterministic Provider Config Reload On `make start`

- Priority: P1
- Problem: regenerated config can drift from long-running service state if containers are not recreated.
- Target: ensure config-driven services always restart/recreate when configs change.
- Acceptance:
1. operator + monitor always consume latest generated config after `make start`.
2. address drift between `addresses.env`, generated config, and runtime submission target is eliminated.

## CCV-003: End-to-End Regression Matrix (DVN + CCV)

- Priority: P1
- Problem: smoke checks are not enforced in CI yet.
- Target: add provider-smoke coverage for setup/start/send/watch/stop.
- Acceptance:
1. CI executes DVN and CCV smoke paths.
2. stale-pending retry behavior remains covered.

## Recently Completed

1. Watch flow now requires destination on-chain confirmation:
- `Relayer: confirmed` alone is not considered success.
- `MessageExecuted(messageId)` must be present in destination tx receipt.
2. Watch flow now exits early on terminal `Failed` submission state with explicit diagnostics.

## Notes

- Broader platform cleanup remains tracked in `FUTURE_IMPROVEMENTS.md`.
