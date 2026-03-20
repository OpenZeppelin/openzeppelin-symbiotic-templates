# Provider Platform Spec

Status: Draft
Date: 2026-02-05

## 1. Purpose

Define how the operator becomes a reusable provider platform so this repository can support multiple protocol integrations with minimal core changes.

This spec is the architecture contract for:
1. Existing provider: LayerZero DVN.
2. Next provider: Symbiotic CCV (CCIP-compatible verifier path).
3. Future providers: additional protocols with the same shared operator infrastructure.

## 2. Problem Statement

The codebase already has a provider interface for ingress (`handle_webhook_event`) and route registration, but signing and submission paths are still tightly DVN-coupled.

Observed coupling points:
1. DVN-specific payload and merkle logic in signer:
- `operator/src/signer/mod.rs`
2. DVN-specific submission ABI and calldata in submitter:
- `operator/src/relay_submitter/mod.rs`
- `operator/src/submitter/dvn.rs`
3. DVN naming in relayer config (`dvn_address`) instead of protocol-neutral target:
- `operator/src/config/mod.rs`
- `operator/src/relayer_client/types.rs`

## 3. Goals

1. Keep monitor/relay/relayer/runtime loops shared.
2. Isolate protocol semantics behind provider adapters.
3. Make adding provider #N require local provider code plus registration, not core-loop rewrites.
4. Keep DVN behavior intact while introducing CCV.
5. Keep CCV default path Symbiotic-only (no dependency on Chainlink auxiliary devenv services).

## 4. Non-Goals

1. Rewriting all storage tables in one step.
2. Full CCV external stack parity in this spec (indexer/aggregator/executor services).
3. Building provider-specific business logic in shared orchestration loops.

## 5. Design Principles

1. Shared transport and orchestration; isolated protocol semantics.
2. One way through the system: ingest -> sign/build -> submit.
3. Provider-specific code must not leak into generic loops.
4. New provider integration should be checklist-driven and testable.

## 6. Platform Model

### 6.1 Provider Bundle

Each provider registers a bundle of adapters:
1. Ingress adapter.
2. Signing adapter.
3. Submission adapter.
4. Optional provider API routes.

### 6.2 Shared Runtime

Generic jobs remain shared:
1. Ingress handling via webhook route + provider dispatch.
2. Signer worker pool and retry loop.
3. Relayer submission loop and status polling.

## 7. Provider Interfaces

### 7.1 IngressAdapter

Responsibility:
1. Convert protocol-specific ingress payloads into normalized stored messages.
2. Validate protocol event shape and destination support.

Interface contract:
1. Input: `WebhookEvent` (or future ingress source adapter output).
2. Output: persisted `MessageData` entries with deterministic `message_id`.

Current mapping:
1. Existing `Provider::handle_webhook_event(...)` already satisfies this role.

### 7.2 SigningAdapter

Responsibility:
1. Define grouping strategy for pending messages.
2. Build bytes that are sent to Symbiotic Relay for signing.
3. Finalize provider-specific artifact from relay response.

Interface contract:
1. Input: normalized messages and provider config.
2. Output:
- signing work items
- provider artifact state sufficient for submission

Examples:
1. DVN: merkle-root-oriented artifact.
2. CCV: message/verifier-result-oriented artifact.

### 7.3 SubmissionAdapter

Responsibility:
1. Build destination transaction call data for a message/artifact.
2. Provide destination target address and idempotency key inputs.

Interface contract:
1. Input: message + artifact + provider config.
2. Output: protocol-agnostic relayer request payload (`to`, `data`, `value`, `gas_limit`, `idempotency_key`).

Examples:
1. DVN: `submitProof(...)`.
2. CCV: `OffRamp.execute(...)`.

### 7.4 ProviderConfigAdapter

Responsibility:
1. Validate provider-specific config blocks.
2. Prevent protocol assumptions from leaking into shared config structs.

Interface contract:
1. Input: provider config section.
2. Output: validated typed provider config.

## 8. Canonical Runtime Data Contracts

The platform uses common runtime entities regardless of provider:
1. `Message` (existing `MessageData`) with provider-owned payload bytes.
2. `Artifact` (new provider-owned signing/submission artifact record).
3. `SubmissionStatus` (existing relayer submission tracking).

Minimum requirements:
1. Message identity must be deterministic and protocol-correct.
2. Artifact must contain enough provider data to build submission calldata deterministically.
3. Submission idempotency key must be deterministic for message + artifact context.

## 9. Module Layout

Target layout (conceptual):
1. `operator/src/provider/mod.rs`
- provider registration and shared trait exports
2. `operator/src/provider/layerzero/*`
- ingress, signing, submission adapters for DVN
3. `operator/src/provider/chainlink_ccv/*`
- ingress, signing, submission adapters for CCV
4. `operator/src/signer/*`
- generic signer loop calling `SigningAdapter`
5. `operator/src/relay_submitter/*`
- generic submit loop calling `SubmissionAdapter`

## 10. Config Model

### 10.1 Provider Selection

1. Keep top-level `provider` selector.
2. Add provider-specific config blocks (e.g. `layerzero`, `chainlink_ccv`).

### 10.2 Relayer Targets

Rename relayer chain target field to protocol-neutral naming:
1. Current: `dvn_address`
2. Target: `target_address`

The submitter asks the provider adapter how to interpret/use the target.

## 11. Migration Plan

### Phase 1: Interface Extraction (No Behavior Change)

1. Introduce signing and submission adapter traits.
2. Implement DVN adapters using current logic.
3. Keep current runtime behavior identical.

### Phase 2: Core Loop Decoupling

1. Refactor `SignerJob` to call `SigningAdapter`.
2. Refactor `RelaySubmitterJob` to call `SubmissionAdapter`.
3. Keep existing ingestion interface unchanged.

### Phase 3: CCV Provider Implementation

1. Add CCV ingress adapter.
2. Add CCV signing adapter.
3. Add CCV submission adapter.
4. Add CCV provider config validation.

### Phase 4: Conformance Hardening

1. Add provider conformance tests.
2. Require conformance suite pass for all providers.

## 12. Provider Conformance Requirements

Every provider must satisfy:
1. Deterministic message ID derivation.
2. Idempotent ingestion behavior.
3. Deterministic signing payload/artifact generation.
4. Deterministic submission request generation.
5. Correct retry behavior under transient relay/relayer failures.
6. Observability parity (core logs/metrics fields).

## 13. Acceptance Criteria

1. DVN behavior remains unchanged after interface extraction.
2. Core signer/submitter loops contain no protocol-specific branching.
3. Adding CCV does not require modifying shared loop semantics.
4. A third provider can be added by implementing adapters and registration only.

## 14. Risks and Mitigations

1. Risk: over-generalization before second provider ships.
- Mitigation: keep interfaces minimal and derived from DVN + CCV only.
2. Risk: storage migration complexity.
- Mitigation: introduce additive artifact records first, migrate gradually.
3. Risk: config churn breaks templates.
- Mitigation: phase config renames with explicit validation and template updates.

## 15. Follow-On Specs

1. Smart Contract Spec (CCV verifier contracts and resolver wiring).
2. Operator Spec (file-by-file code changes in provider/signer/submitter/config/storage).
3. Deployment/Devnet Spec (scripts, templates, local mocks).
4. Test Spec (conformance suite plus integration matrix).
5. Observability Spec (logs/metrics/troubleshooting updates).

## 16. References

1. `docs/symbiotic-ccv-mvp-spec.md`
2. `devdocs/ccip-verifier-overview.md`
3. `docs/architecture.md`
4. `operator/src/provider/mod.rs`
5. `operator/src/signer/mod.rs`
6. `operator/src/relay_submitter/mod.rs`
7. `operator/src/submitter/dvn.rs`
8. `operator/src/config/mod.rs`
