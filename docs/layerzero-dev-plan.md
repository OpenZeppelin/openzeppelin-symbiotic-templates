## Summary
- Ship a turnkey template to spin up Symbiotic-secured LayerZero DVN + example OApp in one command (devnet + Foundry tests + off-chain worker), mirroring super-sum UX but for LayerZero.

## Requirements
- Functional: deploy Symbiotic core, DVN, ReceiveUln/OApp demo; assign/verify LayerZero jobs end-to-end via worker + sidecar; emit deploy-data JSON; CLI one-liner (`generate + compose up + bootstrap`) to run devnet; Foundry tests covering Endpoint→DVN→ReceiveUln; PacketSent-driven worker with sidecar proof submission; per-dst fee/config; MessageLib-only `assignJob`.
- Non-functional: minimal steps, clear docs, deterministic addresses/artifacts, mock-proofs fallback, runnable offline (no external RPC), Docker-first but also pure-Foundry path.

## Technical Approach
- Reuse super-sum infra: generator script produces compose, sidecar configs, deploy-data mounts; deployer container runs Foundry bootstrap script that writes JSON used by worker/monitor.
- Contracts: enhance DVN (MessageLib gating, dst configs, fee calc), add mocks (MessageLib/Endpoint/ReceiveUln) + sample OApp + DVN factory/middleware stubs.
- Off-chain: Rust (existing) or TS worker listens PacketSent/DVNFeePaid, reconstructs payload hash, queries sidecar (HTTP/grpc) for aggregation proof with mock fallback, calls `submitVerification`.
- Devnet: two anvils (source/dest=settlement), relay aggregator + sidecars, dvn-worker, optional OZ monitor containers; `generate_layerzero_network.sh` builds temp-network compose and configs; `up.sh` runs compose + `forge script DevnetBootstrap`.
- Tests: Foundry integration using LZ devtools; worker unit test for packet parsing/proof submission; smoke e2e script in devnet CI.

## Task Breakdown
- Setup
  - Port super-sum generator to `devnet/generate_layerzero_network.sh` (opts: operators/aggregators) and configs.
  - Flesh `DevnetBootstrap.s.sol` to deploy Symbiotic core, DVN, mocks, OApp; write `devnet/deploy-data/*.json`.
- Core development
  - DVN: MessageLib access control, per-dst config/fee, resubmit protection, events; ReceiveUln call path.
  - Add DVN factory, middleware/resolver stubs; OApp example + wiring script (`WireOApp.s.sol`).
  - Worker: implement PacketSent parsing, sidecar proof fetch, submitVerification; retries/timeout; mock mode.
  - Compose: add deployer container, healthchecks, env wiring for worker/sidecars/OZ monitor.
- Testing
  - Foundry: endpoint→DVN→ReceiveUln happy path, fee/expiry, auth reverts.
  - Worker unit/integration (PacketSent → submitVerification) with mock sidecar.
  - Devnet smoke: script to send OApp message and assert delivery.
- Docs
  - Single quickstart (one-command devnet + pure-Foundry path), env table, troubleshooting; deprecate duplicate plans.
- Deployment polish
  - Deterministic addresses via salts, write deploy-data JSON for monitors/scripts; cleanup down.sh.

## Dependencies
- LayerZero devtools/test libraries; Symbiotic relay images/sidecar endpoints; Docker; Foundry; bun/TS or Rust toolchain for worker.

## Risks & Mitigations
- Sidecar availability → mock-proof fallback + healthcheck gating.
- Payload hash mismatch with LZ spec → use official devtools + test vectors.
- Compose drift/port conflicts → generator parameterizes ports, healthchecks.
- Long bootstrap time → parallel container builds, cached images, minimal npm/bun steps.

## Estimated Timeline
- Setup scripts + bootstrap: 1d
- DVN contract hardening + tests: 1–1.5d
- Worker implementation + tests: 1–1.5d
- Devnet compose + smoke test: 1d
- Docs cleanup: 0.5d
- Total ~4–5d focused.

## Open Questions
- Worker language final choice (Rust vs TS) and runtime in compose?
- Need OZ monitor in first cut or optional?
- Exact fee model per dst (flat vs gas price oracle)?
- Do we include middleware/slashing flow now or stub for later?
