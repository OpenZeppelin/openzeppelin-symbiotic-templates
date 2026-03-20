# Operator MVP Spec (Single Active Provider)

Status: Draft  
Date: 2026-02-05

## 1. Purpose

Define the minimum operator code changes required to support:
1. Existing LayerZero DVN provider.
2. New Symbiotic CCV provider.
3. Single active provider per running stack.

This spec is implementation-focused and file-level.

## 2. Scope

In scope:
1. Provider seams for ingress, signing, and submission.
2. Config/schema changes needed by operator runtime.
3. Minimal storage/runtime changes to support CCV alongside DVN.
4. Tests needed to keep DVN stable while adding CCV.

Out of scope:
1. Full compile-time provider feature gating (tracked in `FUTURE_IMPROVEMENTS.md`).
2. Multi-active-provider runtime in one process.
3. Chainlink CCV auxiliary devenv parity in local dev (explicitly out of scope for the Symbiotic-only template path).

## 3. Current Operator Constraints

Current abstraction state:
1. `Provider` abstraction exists for ingress and API routes:
- `operator/src/provider/mod.rs`
2. Signer path is DVN-specific:
- merkle batching, DVN leaf/hash assumptions in `operator/src/signer/mod.rs`
3. Submitter path is DVN-specific:
- DVN calldata encoding in `operator/src/relay_submitter/mod.rs`
- `operator/src/submitter/dvn.rs`
4. Relayer config uses DVN naming:
- `dvn_address` in `operator/src/config/mod.rs`
- `dvn_address` in `operator/src/relayer_client/types.rs`

## 3.1 Current vs Proposed Model

Current model (DVN-shaped in core loops):
1. Shared signer loop computes DVN merkle artifacts directly.
2. Shared submitter loop builds DVN `submitProof(...)` calldata directly.
3. Storage and flow assumptions are rooted in merkle-root DVN lifecycle.

Proposed model (provider-owned protocol semantics):
1. Shared loops keep orchestration only (poll, retry, relay/relayer I/O, status transitions).
2. Provider implementation owns:
- signing payload construction
- artifact payload format
- destination calldata construction
3. Shared storage keeps provider-neutral envelope fields with provider-owned payload bytes.

## 4. MVP Design

## 4.1 Single Runtime Provider

Runtime keeps one provider selected by config:
1. `provider` field in operator config is authoritative.
2. `create_provider(...)` returns that provider only.
3. Signer and submitter are provider-dispatched, not hard-coded DVN.

## 4.2 Provider Seams

Extend provider module so runtime jobs call provider-owned logic.

Minimum seam contracts:
1. Ingress seam:
- keep existing `handle_webhook_event(...)`.
2. Signing seam:
- provider decides how pending messages are grouped and what artifact/signing payload is produced.
- for CCV, signing output must include Symbiotic attestation material needed by verifier results.
3. Submission seam:
- provider builds destination transaction request data (`to`, `data`, optional gas/value, idempotency key).

MVP rule:
1. Shared jobs remain orchestration loops.
2. Provider owns protocol math and ABI payload construction.

## 4.3 Config Neutralization

Rename relayer mapping field:
1. From `dvn_address`
2. To `target_address`

Interpretation:
1. For LayerZero DVN, `target_address` is DVN contract address.
2. For CCV, `target_address` is provider-specific submit target (for `OffRamp.execute(...)` path this is expected to be OffRamp address).

## 4.4 Artifact Persistence (MVP)

MVP requirement:
1. Persist provider-specific signing/submission artifacts so restart/retry remains deterministic.

Minimum acceptable approach:
1. Add a provider artifact table keyed by deterministic artifact ID.
2. Store serialized provider artifact bytes + metadata (`provider`, chain ids, message ids, status).

Avoid in MVP:
1. Deep storage redesign of all existing DVN tables.

Important clarification:
1. CCV may still rely on Symbiotic BLS attestation/proof material.
2. Even if both providers use BLS, artifact encoding is not assumed to be identical.
3. DVN artifact encoding (`submitProof` path) and CCV verifier-result encoding (`OffRamp.execute` path) remain provider-specific payloads.
4. In CCV mode, attestation verification by `SymbioticCCV.verifyMessage(...)` is a required security check, not optional post-processing.

## 5. File-Level Change Plan

## 5.1 Config and Relayer Types

1. `operator/src/config/mod.rs`
- rename `ChainRelayerEntry.dvn_address` -> `target_address`.
2. `operator/src/relayer_client/types.rs`
- rename `ChainRelayerConfig.dvn_address` -> `target_address`.
3. `operator/src/main.rs`
- wire renamed field through `ChainRelayerConfig::new(...)`.

## 5.2 Provider Module

1. `operator/src/provider/mod.rs`
- define signing/submission adapter traits or equivalent trait methods.
- keep provider registration as single factory point.
2. `operator/src/provider/layerzero.rs`
- adapt to new seam methods using current DVN behavior.
3. `operator/src/provider/chainlink_ccv.rs` (new)
- ingress decoding for CCV event path.
- MVP signing artifact builder.
- MVP submission request builder for OffRamp execute path.

## 5.3 Signer Loop

1. `operator/src/signer/mod.rs`
- remove direct DVN assumptions from shared loop.
- delegate grouping/artifact build/sign-payload handling to provider seam.
- keep worker, retry, and shutdown orchestration shared.

## 5.4 Relay Submitter Loop

1. `operator/src/relay_submitter/mod.rs`
- remove direct imports of `compute_dvn_leaf`, `DecodedJobAssigned`, `encode_submit_proof` from shared path.
- ask provider seam for submission request payload per message/artifact.
- keep relayer submission and status polling loop shared.
2. `operator/src/submitter/dvn.rs`
- keep as LayerZero provider-owned helper module in MVP.

## 5.5 Storage

1. `operator/src/storage/mod.rs`
- add minimal provider artifact persistence APIs.
- preserve existing message/submission APIs where possible.

## 6. MVP Acceptance Criteria

1. LayerZero DVN flow behavior remains unchanged.
2. CCV flow can ingest, sign, and submit via provider seam path.
3. Shared loops contain no protocol-specific branching for DVN vs CCV payload math.
4. Restart/retry does not produce duplicate inconsistent submissions.
5. `target_address` rename is applied end-to-end in operator runtime config usage.
6. CCV end-to-end tests prove Symbiotic attestation is actually verified on-chain in `verifyMessage(...)`.

## 7. Test Plan (MVP)

1. Unit tests:
- provider factory and config validation for both providers.
- `target_address` config wiring.
- provider seam deterministic outputs.
2. Integration tests:
- DVN happy path regression.
- CCV happy path with Symbiotic-only mock ramp path.
3. Failure/retry tests:
- relayer transient failure retries.
- process restart with pending artifacts.

## 8. Key Concerns

1. Ingress shape mismatch:
- LayerZero currently ingests `JobAssigned`, CCV ingress is different; shared monitor payload assumptions must not leak into core parsing.
2. Artifact model risk:
- if artifact schema is too DVN-shaped, CCV path will force ad hoc hacks.
3. Relayer target ambiguity:
- `target_address` must be interpreted consistently by each provider; otherwise submissions can silently point to wrong contracts.
4. Idempotency key strategy:
- must include provider + message/artifact identity to avoid cross-provider collisions.

## 9. Follow-On Cleanup (After MVP)

1. Move provider-specific helper modules under provider directories fully.
2. Reduce legacy DVN names in logs/metrics.
3. Add compile-time provider feature gating and slim provider-specific builds.
