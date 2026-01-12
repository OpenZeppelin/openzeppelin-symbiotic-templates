[1. Decision Log](https://www.notion.so/2c7cbd12786080a59406e3d5024589a7?pvs=21)

## 2. Milestone Details

### System Overview

OpenZeppelin proposes a comprehensive template for integrating LayerZero DVN with Symbiotic. This template allows developers to rapidly and securely deploy LayerZero DVNs leveraging Symbiotic’s economic security guarantees. By building upon OpenZeppelin Monitor, the template offers minimal configuration requirements.

- Easily deploy operator infrastructure using OpenZeppelin Monitor configurations.
- Robust developer tools for streamlined local testing and iterative development.
- Extensible architecture supporting future integrations with additional bridges such as Hyperlane.

We will provide a template that:

- Securely verifies LayerZero messages on destination chains using Symbiotic Settlement module.
- Offers operators a straightforward, off-chain workflow powered by OpenZeppelin Monitor alongside lightweight Rust worker processes.
- Equips network administrators with dedicated monitoring tools to detect anomalies and trigger the Slasher module when necessary.

---

## Components

### On-chain: SymbioticLayerZeroDVN

A DVN contract deployed on both source and destination chains that integrates LayerZero’s verification interface with Symbiotic’s BLS quorum proof verification.

### Off-chain: Operator template using OpenZeppelin Monitor (Docker)

Monitors DVN `JobAssigned` events on source chains, requests Symbiotic relay signatures via gRPC, polls for aggregated BLS proof, and submits verification to destination chain DVN.

---

## On-chain: SymbioticLayerZeroDVN

**Interface:** Implements LayerZero’s `ILayerZeroDVN` with `assignJob()` and `getFee()` for source chain operations, plus `submitVerification()` for destination chain verification.

**Validation model:**

- On source chain: `assignJob()` is called by SendUln302, storing job details and emitting `JobAssigned` event for off-chain operators.
- On destination chain: `submitVerification()` extracts the signed payload (`keccak256(packetHeader, payloadHash)`), verifies BLS quorum proof via Settlement contract, then calls `ReceiveUln302.verify()` to notify LayerZero.

**Source Chain Functions:**

| Function                                         | Description                                                                |
| ------------------------------------------------ | -------------------------------------------------------------------------- |
| `assignJob(AssignJobParam, options)`             | Called by SendUln302 to assign verification job; emits `JobAssigned` event |
| `getFee(dstEid, confirmations, sender, options)` | Returns fee for verification job                                           |

**Destination Chain Functions:**

| Function                                                                     | Description                                                          |
| ---------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `submitVerification(packetHeader, payloadHash, confirmations, epoch, proof)` | Verifies Symbiotic BLS quorum proof and calls ReceiveUln302.verify() |

---

## Off-chain: Operator (OpenZeppelin Monitor)

We use OpenZeppelin Monitor as an event-driven, no-code (JSON) monitor with custom triggers. The operator packs all logic in a Rust worker invoked by Monitor.

**Each operator runs:** (1) OpenZeppelin Monitor + (2) Symbiotic Relay Sidecar + (3) DVN Worker

**Workflow:**

1. Watch source chain for DVN `JobAssigned` events.
2. On event detection, invoke Rust worker that:
   - Computes message hash: `keccak256(abi.encode(packetHeader, payloadHash))`
   - Requests BLS signature from local Relay Sidecar via gRPC
   - Polls for aggregated proof (2/3+ quorum)
   - Submits `submitVerification()` to destination chain DVN

**Why OZ Monitor:**

- JSON-only config → easy to distribute per network.
- Works multi-chain and supports custom workers via trigger hooks.
- Flexible Setup: Running via Docker or as a standalone CLI binary depending on the operator environment setup.

**Configuration model (all JSON):**

- `networks/*.json`: chain endpoints and scheduling.
- `monitors/*.json`: what to watch (DVN address, `JobAssigned` event) and which triggers to run.
- `triggers/*.json`: DVN worker script invocation with timeout configuration.

**Runtime:** Custom Docker image extending `openzeppelin/openzeppelin-monitor` with Rust DVN worker binary; bind-mount configs and deploy-data.

---

## Message Flow

Source chain event → destination chain delivery:

```
1. OApp calls EndpointV2.send() on source chain
2. SendUln302 calls DVN.assignJob() with packet data
3. DVN emits JobAssigned event (configured in monitors/*.json)
4. OZ Monitor detects event and invokes worker script
5. Worker computes messageHash = keccak256(packetHeader, payloadHash)
6. Worker requests BLS signature from local Relay Sidecar (gRPC)
7. Worker polls for aggregated proof (retry with backoff)
8. Worker submits DVN.submitVerification() on destination chain
9. DVN verifies proof via Settlement.verifyQuorumSigAt()
10. DVN calls ReceiveUln302.verify() to notify LayerZero
11. Message is marked as verified and delivered to destination OApp
```

---

## Template Structure

```
symbiotic-layerzero-template/
├── .env.example                          # Central configuration
├── config/                               # Template configuration files
│   ├── chains.json                       # Chain definitions
│   └── dvn.json                          # DVN parameters
├── src/
│   ├── SymbioticLayerZeroDVN.sol         # Core DVN contract
│   ├── symbiotic/                        # Symbiotic contract wrappers
│   │   ├── Settlement.sol
│   │   ├── Driver.sol
│   │   ├── KeyRegistry.sol
│   │   └── VotingPowers.sol
│   └── examples/
│       └── TestOApp.sol                  # Example OApp (delete in prod)
├── script/
│   ├── SourceChainDeploy.s.sol           # Phase 1: Source chain
│   ├── DestinationChainDeploy.s.sol      # Phase 2: Dest chain
│   ├── DriverDeploy.s.sol                # Phase 3: Driver
│   ├── LayerZeroSourceDeploy.s.sol       # Phase 4: LZ Source
│   ├── LayerZeroDestDeploy.s.sol         # Phase 5: LZ Dest
│   └── examples/
│       └── TestOAppDeploy.s.sol
├── test/
│   ├── SymbioticLayerZeroDVN.t.sol       # Unit tests
│   └── Integration.t.sol                 # E2E tests
├── off-chain/
│   ├── operator/                         # Operator infrastructure
│   │   ├── Dockerfile
│   │   ├── entrypoint.sh
│   │   ├── config/
│   │   │   ├── monitors/
│   │   │   ├── networks/
│   │   │   └── triggers/
│   │   └── worker/                       # Rust DVN worker
│   │       └── src/
│   │           ├── main.rs
│   │           ├── sidecar.rs            # gRPC client
│   │           └── contracts.rs
│   └── network-owner/                    # Owner monitoring configs
├── devnet/
│   ├── devnet.sh                         # Management CLI
│   ├── docker-compose.yml                # Full devnet stack
│   └── deploy-data/                      # Generated artifacts
├── docs/
│   ├── QUICK_START.md
│   ├── CONFIGURATION.md
│   ├── DEPLOYMENT.md
│   └── OPERATOR_GUIDE.md
└── README.md
```

---

## Devnet Environment

The template includes a complete local development environment:

| Service                 | Purpose                                |
| ----------------------- | -------------------------------------- |
| `anvil-source`          | Source chain (31337)                   |
| `anvil-dest`            | Destination chain (31338)              |
| `deployer`              | Multi-phase contract deployment        |
| `genesis-generator`     | Initialize Symbiotic validator set     |
| `relay-sidecar-{1,2,3}` | Symbiotic relay nodes with BLS signing |
| `dvn-operator-{1,2,3}`  | OZ Monitor + DVN worker per operator   |

**Commands:**

```bash
./devnet.sh up [--fresh]    # Start devnet
./devnet.sh test            # Run E2E test
./devnet.sh logs [service]  # View logs
./devnet.sh reload <target> # Hot reload contracts
./devnet.sh clean           # Wipe all state
```

### Implementation Plan

## Phase 1: Rust Sidecar

Build the core operator service in Rust, porting architecture from the Go reference implementation.

### Components

- HTTP API (webhook receiver, health endpoints)
- Provider system (LayerZero event parsing, message storage)
- Database layer (message batching, Merkle tree storage)
- Signer job (relay integration, proof polling)
- Submitter job (destination chain verification)
- Extensible architecture (provider interface)

### API Hardening

- Authentication for webhook endpoints

### Testability

- Mock implementations (relay, chain, database)
- Unit tests with mocked dependencies
- Integration tests with real database

---

## Phase 2: Devnet Optimization

Improve developer experience with faster iteration cycles.

### Optimizations

- Consolidated deployment script (reduce forge overhead)
- Parallel service startup where possible
- Pre-deployed state snapshots for fast restarts
- Isolated testing modes (`-relay-only`, `-sidecar-only`)

---

## Phase 3: Configuration & Deployment

Centralize configuration and enable testnet deployment.

### Configuration

- JSON config schema with validation
- Environment-specific templates (devnet, testnet, mainnet)
- Config loading in sidecar and deploy scripts

### Deployment

- Standalone DVN deploy script (doesn't require full Symbiotic deployment)
- Contract verification integration
- Testnet docker-compose for operators

### Multi-Chain Orchestration

- Unified deployment pipeline (devnet → testnet → mainnet)
- Deterministic addresses via CREATE2 where applicable
- Cross-chain deployment scripts with rollback support
- Chain-specific configuration templating

---

## Phase 4: Documentation & Polish

Complete the template for external use.

### Documentation

- Quick start guide
- Configuration reference
- Deployment guide
- Operator guide

### Template Cleanup

- Example vs core code separation
- Extension point markers (`// CUSTOMIZE:`)
- Example configs for common chains

### CI/CD & Scaffolding

- GitHub Actions workflows (test, lint, security scan)
- Standard template files (LICENSE, CONTRIBUTING.md, SECURITY.md)
- Pre-commit hooks configuration

### Testing Plan

1. **Unit Tests**
   - Contract logic
   - Sidecar components
   - Minimum **95%** coverage for core contracts
2. **Integration Tests**
   - Sidecar with real database, mocked external services
   - Contract-to-contract interactions
3. **E2E Tests**
   - Full devnet message flow
   - Failure recovery (operator restart, relay timeout)

### Risks & Blockers

## 3. Acceptance Criteria

**Date of Acceptance:** Dec 23rd, 2025

**Reviewed by:** Soumya

**Approved by (Customer):** Soumya

- **Acceptance Checklist:**
  - [ ] **GitHub repository is configured** with 2-review and 95% coverage rules.
  - [ ] **Release & Upgrade process documentation** is delivered and accepted.
  - [ ] **On-Chain Contracts are delivered** with all functions as specified (DVN, Symbiotic Wrappers, TestOApp).
  - [ ] **Off-Chain Infrastructure is complete** (Rust Sidecar, HTTP API, Merkle Storage) and packaged in the operator Docker image.
  - [ ] **Deployment scripts are complete** for full Devnet contract lifecycle and multi-chain orchestration.
  - [ ] **Devnet Environment is fully operational** including the multi-service docker-compose and unified CLI.
  - [ ] **Tests / audit passed:** All code is peer-reviewed (2+) and meets the 95% code coverage requirement. (link: [paste link here])
  - [ ] **Documentation delivered** (in-code comments, READMEs, and the four required Markdown guides).

## 4. Final Sign-off

- Customer Approver: [ ]
- Date: [ ]
- Status: ⬜ Pending | ✅ Approved
