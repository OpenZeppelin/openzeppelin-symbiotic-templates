# Architecture

System overview for the Symbiotic multi-provider template.

## Core Model

1. One active provider per running stack (`config/root.config.json`).
2. Shared off-chain runtime:
- OZ Monitor for ingress
- 3 operator processes
- 3 Symbiotic relay sidecars for BLS signatures
- OZ Relayer for destination tx submission
- Redis queue
3. Provider-specific on-chain contracts and calldata format.

## Provider Matrix

| Provider | Source ingress event | Destination submit call | Destination verification condition |
| --- | --- | --- | --- |
| `layerzero` | `JobAssigned` | `SymbioticLayerZeroDVN.submitProof(...)` | Destination target verification/forward path |
| `chainlink_ccv` | `CCIPMessageSent` | `OffRamp.execute(...)` | `MessageExecuted(messageId)` + `SymbioticCCV.verifyMessage(...)` |

## CCV Scope Assumption

This template supports the **Symbiotic CCV variant** only.

Not in local scope:
1. Chainlink CCV auxiliary devenv stack (`aggregator`, `indexer`, `verifier`, `executor`).
2. External protocol attestation providers (for example CCTP token attestors).

## LayerZero Flow

1. Source emits `JobAssigned`.
2. Monitor forwards webhook payloads to operators.
3. Operators batch messages into Merkle roots and collect BLS signatures via sidecars.
4. Relayer submits proof to destination DVN contract.
5. Destination DVN verifies quorum through settlement and continues protocol flow.

## Symbiotic CCV Flow

1. Source OnRamp-compatible contract emits `CCIPMessageSent`.
2. Monitor forwards event payloads to operators.
3. Operators build CCV payload and collect Symbiotic BLS attestation.
4. Relayer submits destination `OffRamp.execute(...)`.
5. OffRamp-compatible destination contract calls `SymbioticCCV.verifyMessage(...)` for each supplied CCV.
6. On success destination emits `MessageExecuted(messageId,...)`.

`scripts/msg watch` for `chainlink_ccv` treats success only when destination `MessageExecuted` is found on-chain.

## BLS Role (Both Providers)

1. Operators sign provider-defined payloads through Symbiotic relay sidecars.
2. Aggregation/quorum logic comes from settlement-backed Symbiotic attestation rules.
3. Provider-specific contracts decode and enforce those attestations on destination execution path.

## Development Topology

```text
Source chain (31337)                         Destination chain (31338)
--------------------                         ------------------------
LayerZero: JobAssigned                       LayerZero: DVN.submitProof verify path
CCV:      CCIPMessageSent                    CCV:      OffRamp.execute -> SymbioticCCV.verifyMessage

              OZ Monitor -> Operators -> Symbiotic Relays -> OZ Relayer
                                (shared off-chain runtime)
```
