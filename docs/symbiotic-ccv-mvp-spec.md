# Symbiotic CCV MVP Spec

Status: Draft
Date: 2026-02-05

## 1. Purpose

Define the MVP scope for adding a Symbiotic CCV integration (CCIP-compatible verifier path) as a new provider in this repository.

## 2. Strategic Goal (Project-Level)

This repo is a template platform for deploying Symbiotic-powered integrations across multiple protocols.

Current and planned path:
1. Existing provider: LayerZero DVN.
2. Next provider: Symbiotic CCV.
3. Future providers: additional protocols using the same operator/infra foundation.

Therefore, a major goal of this work is to make adding new providers easy, predictable, and low-risk.

## 3. Scope

### In Scope (MVP)

1. Add a `SymbioticCCV` provider path while preserving existing DVN behavior.
2. Reuse shared infrastructure where possible:
- OZ Monitor
- Symbiotic Operator runtime
- Symbiotic Relay sidecars
- OZ Relayer
3. Implement CCV-specific semantics:
- Source ingress from `CCIPMessageSent`
- Destination submission via `OffRamp.execute(...)`
- Verifier path via CCV resolver + `verifyMessage(...)`
4. Use a Symbiotic-only local mock path for CCV ramps (`MockCCIPOnRamp`, `MockCCIPOffRamp`) while keeping monitor/operator/relay/relayer flow intact.

### Out of Scope (MVP)

1. Chainlink CCV auxiliary devenv services (aggregator/indexer/executor stack).
2. Token-specific external attestation integrations (CCTP/Lombard style) in v1.
3. Unrelated refactors outside provider platform boundaries.

## 4. Confirmed Architecture Direction

Reuse infrastructure topology, change protocol semantics.

MVP operational path:
1. `OnRamp` emits `CCIPMessageSent`.
2. OZ Monitor forwards event data to operators.
3. Operator builds CCV-specific payload and obtains Symbiotic BLS attestation via Symbiotic Relay.
4. Operator prepares `verifierResults` that include Symbiotic attestation data and destination calldata.
5. OZ Relayer submits `OffRamp.execute(encodedMessage, ccvs, verifierResults, gasLimitOverride)`.
6. OffRamp resolves inbound verifier implementation and calls `verifyMessage(...)`.
7. In local mock mode, destination OffRamp mock executes `SymbioticCCV.verifyMessage(...)` on-chain before marking execution complete.

## 5. Key Protocol Constraints

1. CCV does not use DVN `JobAssigned` semantics.
2. CCV submission is not DVN `submitProof(...)`; must use OffRamp `execute(...)`.
3. `ccvs` and `verifierResults` must align by length and ordering expected by OffRamp checks.
4. `messageId` must be consistent with OffRamp derivation from encoded message.
5. Symbiotic security is mandatory for this template integration: CCV verifier results must carry Symbiotic BLS attestation material.
6. `SymbioticCCV.verifyMessage(...)` must validate Symbiotic attestation against settlement/quorum rules before accepting the message as verified.

## 6. Provider-Platform Requirement (Design Principle)

All protocol-specific behavior must be isolated behind provider boundaries so we can add future providers with minimal core churn.

Minimum platform seams expected after CCV:
1. Ingress adapter (event source + normalization).
2. Signing/verification payload strategy.
3. Submission adapter (destination ABI call construction).
4. Provider-scoped config model.
5. Provider conformance tests.

## 7. MVP Acceptance Criteria

1. Existing DVN flow remains functional.
2. CCV mode executes one complete source-to-destination flow through `OffRamp.execute(...)`.
3. CCV verifier path (`verifyMessage(...)`) is exercised in tests.
4. CCV verifier path validates Symbiotic BLS attestation in tests (not only payload parsing).
5. Operator retry/idempotency behavior remains safe under restart/replay.
6. Symbiotic-only mock path does not require Chainlink auxiliary services to run end-to-end locally.

## 8. Follow-On Specs (Next)

1. Provider Platform Spec (cross-provider abstractions and extension points).
2. Smart Contract Spec (`SymbioticCCV`, resolver wiring, verifier result format).
3. Operator Spec (provider module, signer strategy, submitter path, config).
4. Deployment/Devnet Spec (scripts, templates, mocks, environments).
5. Test Spec (unit/integration matrix and acceptance tests).

## 9. References

1. `devdocs/CCIP-Symbiotic Doc.md`
2. `devdocs/ccip-verifier-overview.md`
3. `docs/architecture.md`
4. `devdocs/architecture-compare-playground.html`
