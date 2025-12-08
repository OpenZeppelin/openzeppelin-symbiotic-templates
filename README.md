# Symbiotic LayerZero DVN Template

A template for building a LayerZero DVN (Decentralized Verifier Network) secured by Symbiotic's restaking infrastructure.

## Overview

This repo implements a DVN that uses Symbiotic's BLS-BN254 quorum signature verification for LayerZero cross-chain message attestation. Instead of relying on a centralized verifier, the DVN leverages Symbiotic's validator set to achieve decentralized, cryptographically-verified message confirmation.

## Architecture

### System Overview

```
SOURCE CHAIN (31337)                                      DESTINATION CHAIN (31338)
┌────────────────────────────────────┐                   ┌────────────────────────────────────┐
│           LAYERZERO                │                   │           SYMBIOTIC                │
│  ┌─────────┐    ┌──────────────┐  │                   │      ┌─────────────────┐           │
│  │  OApp   │───▶│  EndpointV2  │  │                   │      │   Settlement    │           │
│  └─────────┘    └──────┬───────┘  │                   │      │ (BLS Verify)    │           │
│                        │          │                   │      └────────┬────────┘           │
│                        ▼          │                   │               │                    │
│               ┌─────────────┐     │                   │               ▼                    │
│               │ SendUln302  │     │                   │  ┌────────────────────────┐        │
│               └──────┬──────┘     │                   │  │ SymbioticLayerZeroDVN  │        │
│                      │            │                   │  │   (DEST INSTANCE)      │        │
│                      ▼            │                   │  │                        │        │
│  ┌────────────────────────────┐   │                   │  │  submitVerification()  │        │
│  │  SymbioticLayerZeroDVN     │   │                   │  │    │                   │        │
│  │    (SOURCE INSTANCE)       │   │                   │  │    ├─▶ Settlement      │        │
│  │                            │   │                   │  │    │   .verifyQuorum() │        │
│  │  • assignJob()             │   │                   │  │    │                   │        │
│  │  • getFee()                │   │                   │  │    └─▶ ReceiveUln302   │        │
│  │  • emit JobAssigned        │   │                   │  │        .verify()       │        │
│  └─────────────┬──────────────┘   │                   │  └───────────┬────────────┘        │
│                │                  │                   │              │                     │
├────────────────┼──────────────────┤                   │              ▼                     │
│           SYMBIOTIC               │                   │         LAYERZERO                  │
│  ┌──────────────────────────┐     │                   │  ┌─────────────────┐               │
│  │ Network + KeyRegistry    │     │                   │  │  ReceiveUln302  │               │
│  │ VotingPowers + Driver    │     │                   │  └────────┬────────┘               │
│  └──────────────────────────┘     │                   │           │                        │
└────────────────┼──────────────────┘                   │           ▼                        │
                 │                                      │  ┌─────────────────┐               │
                 │                                      │  │   EndpointV2    │               │
                 │ validator set                        │  └────────┬────────┘               │
                 │ commits each epoch                   │           │                        │
                 │                                      │           ▼                        │
                 └──────────────────────────────────────┼──▶┌─────────────────┐              │
                                                        │   │  OApp (recv)    │              │
                                                        │   │  Message ✓      │              │
                                                        │   └─────────────────┘              │
                                                        └────────────────────────────────────┘
```

### Detailed Message Flow

```
═══════════════════════════════════════════════════════════════════════════════════════════
                              SOURCE CHAIN (31337)
═══════════════════════════════════════════════════════════════════════════════════════════

┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                    LAYERZERO                                             │
│  ┌─────────┐      ┌──────────────┐      ┌─────────────┐                                │
│  │  OApp   │─────▶│  EndpointV2  │─────▶│ SendUln302  │                                │
│  │ (User)  │      │              │      │ (MessageLib)│                                │
│  └─────────┘      └──────────────┘      └──────┬──────┘                                │
│                                                 │                                       │
│                                                 │ assignJob()                           │
│                                                 ▼                                       │
│                                    ┌────────────────────────┐                          │
│                                    │ SymbioticLayerZeroDVN  │                          │
│                                    │    (SOURCE INSTANCE)   │                          │
│                                    │                        │                          │
│                                    │  • assignJob()         │                          │
│                                    │  • getFee()            │                          │
│                                    │  • collect fees        │                          │
│                                    │  • emit JobAssigned    │                          │
│                                    └───────────┬────────────┘                          │
└────────────────────────────────────────────────┼────────────────────────────────────────┘
                                                 │
┌────────────────────────────────────────────────┼────────────────────────────────────────┐
│                                    SYMBIOTIC   │                                        │
│  ┌──────────────────┐  ┌───────────────────┐  │  ┌─────────────────┐                   │
│  │  Symbiotic Core  │  │   KeyRegistry     │  │  │  VotingPowers   │                   │
│  │  (Vaults, Ops)   │  │ (BLS Public Keys) │  │  │ (Stake → Power) │                   │
│  └────────┬─────────┘  └─────────┬─────────┘  │  └────────┬────────┘                   │
│           │                      │            │           │                             │
│           └──────────────────────┼────────────┼───────────┘                             │
│                                  │            │                                         │
│                                  ▼            │                                         │
│                         ┌─────────────────┐   │                                         │
│                         │     Driver      │   │                                         │
│                         │ (ValSet Mgmt)   │───┼─────────────────────────────────────┐   │
│                         └─────────────────┘   │                                     │   │
└────────────────────────────────────────────────┼─────────────────────────────────────┼───┘
                                                 │                                     │
═══════════════════════════════════════════════════════════════════════════════════════════
                              OFF-CHAIN INFRASTRUCTURE
═══════════════════════════════════════════════════════════════════════════════════════════
                                                 │                                     │
                                                 │ JobAssigned event                   │ validator set
                                                 ▼                                     │ commits (each epoch)
┌────────────────────────────────────────────────────────────────────┐                 │
│                         OZ MONITOR                                  │                 │
│            (watches JobAssigned on source DVN)                      │                 │
└───────────────────────────┬────────────────────────────────────────┘                 │
                            │                                                           │
                            │ event data: {jobId, dstEid, packetHeader, payloadHash}   │
                            ▼                                                           │
┌────────────────────────────────────────────────────────────────────┐                 │
│                      RUST DVN WORKER                                │                 │
│                                                                     │                 │
│  1. Receive JobAssigned event                                       │                 │
│  2. Build message: keccak256(packetHeader, payloadHash)            │                 │
│  3. Request BLS signature ─────────────────────────────────────────┼──┐              │
│  4. Wait for aggregation proof ◄───────────────────────────────────┼──┤              │
│  5. Submit to destination chain ───────────────────────────────────┼──┼───┐          │
└────────────────────────────────────────────────────────────────────┘  │   │          │
                                                                        │   │          │
┌───────────────────────────────────────────────────────────────────────┼───┼──────────┼─┐
│                         SYMBIOTIC RELAY                               │   │          │ │
│  ┌─────────────────────────────────────────────────────────────────┐  │   │          │ │
│  │                    RELAY SIDECARS (per operator)                 │  │   │          │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │  │   │          │ │
│  │  │  Sidecar 1  │  │  Sidecar 2  │  │  Sidecar N  │              │◀─┘   │          │ │
│  │  │ (Operator1) │  │ (Operator2) │  │ (OperatorN) │              │      │          │ │
│  │  │ BLS Sign    │  │ BLS Sign    │  │ BLS Sign    │              │      │          │ │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘              │      │          │ │
│  │         └────────────────┼────────────────┘                      │      │          │ │
│  │                          ▼                                       │      │          │ │
│  │                 ┌─────────────────┐                              │      │          │ │
│  │                 │   AGGREGATOR    │                              │      │          │ │
│  │                 │ Collect sigs    │                              │      │          │ │
│  │                 │ until 2/3+      │──────────────────────────────┼──────┘          │ │
│  │                 │ Return proof    │  aggregated BLS proof        │                 │ │
│  │                 └────────┬────────┘                              │                 │ │
│  └──────────────────────────┼───────────────────────────────────────┘                 │ │
│                             │ commit validator set headers                            │ │
│                             └─────────────────────────────────────────────────────────┼─┤
└───────────────────────────────────────────────────────────────────────────────────────┘ │
                                                                                          │
═══════════════════════════════════════════════════════════════════════════════════════════
                              DESTINATION CHAIN (31338)
═══════════════════════════════════════════════════════════════════════════════════════════
                                                                                          │
┌─────────────────────────────────────────────────────────────────────────────────────────┤
│                                    SYMBIOTIC                                            │
│                         ┌─────────────────────────┐                                    │
│                         │      Settlement         │◀───────────────────────────────────┘
│                         │                         │  validator set commits (each epoch)
│                         │ • Stores validator sets │
│                         │ • verifyQuorumSigAt()   │◀─────────┐
│                         │ • BLS-BN254 verification│          │
│                         └─────────────────────────┘          │
└──────────────────────────────────────────────────────────────┼──────────────────────────┘
                                                               │
┌──────────────────────────────────────────────────────────────┼──────────────────────────┐
│                                    LAYERZERO                 │                          │
│  ┌────────────────────────────────────────────┐              │                          │
│  │        SymbioticLayerZeroDVN               │              │                          │
│  │          (DESTINATION INSTANCE)            │◀─────────────┘                          │
│  │                                            │  submitVerification(                    │
│  │  submitVerification():                     │    packetHeader, payloadHash,           │
│  │    1. Build messageHash                    │    confirmations, epoch, proof)         │
│  │    2. settlement.verifyQuorumSigAt() ──────┤                                         │
│  │    3. receiveUln.verify() ─────────────────┼──┐                                      │
│  └────────────────────────────────────────────┘  │                                      │
│                                                  ▼                                      │
│                                    ┌─────────────────┐                                  │
│                                    │  ReceiveUln302  │                                  │
│                                    │  • verify()     │                                  │
│                                    │  • hashLookup[] │                                  │
│                                    └────────┬────────┘                                  │
│                                             │ once all required DVNs verified           │
│                                             ▼                                           │
│                                    ┌─────────────────┐                                  │
│                                    │   EndpointV2    │                                  │
│                                    │ commitVerify()  │                                  │
│                                    └────────┬────────┘                                  │
│                                             │ lzReceive()                               │
│                                             ▼                                           │
│                                    ┌─────────────────┐                                  │
│                                    │      OApp       │                                  │
│                                    │   (Receiver)    │                                  │
│                                    │ Message ✓       │                                  │
│                                    └─────────────────┘                                  │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

### Step-by-Step Flow

| Step | Component | Chain | Action | Data |
|------|-----------|-------|--------|------|
| 1 | OApp | Source | `lzSend()` | message payload |
| 2 | EndpointV2 | Source | Route to MessageLib | packet = header + payload |
| 3 | SendUln302 | Source | `assignJob()` to DVN | packetHeader, payloadHash |
| 4 | DVN (src) | Source | Emit `JobAssigned` | jobId, dstEid, payloadHash |
| 5 | OZ Monitor | Off-chain | Detect event | event data |
| 6 | Rust Worker | Off-chain | Build message | `keccak256(header, payload)` |
| 7 | Sidecars | Off-chain | BLS sign | signature shares |
| 8 | Aggregator | Off-chain | Aggregate signatures | combined BLS sig + bitmap |
| 9 | Rust Worker | Off-chain | Receive proof | epoch, aggregated proof |
| 10 | DVN (dst) | Destination | `submitVerification()` | header, payload, proof |
| 11 | Settlement | Destination | `verifyQuorumSigAt()` | validates BLS proof |
| 12 | ReceiveUln302 | Destination | `verify()` | stores DVN attestation |
| 13 | EndpointV2 | Destination | `commitVerification()` | message ready |
| 14 | OApp | Destination | `lzReceive()` | message delivered |

### Component Responsibilities

| Component | Chain | Role |
|-----------|-------|------|
| **SendUln302** | Source | Routes messages, calls `DVN.assignJob()` |
| **DVN (src)** | Source | Collects fees, emits events for off-chain |
| **Driver** | Source | Manages validator sets, commits to Settlements |
| **KeyRegistry** | Source | Stores operator BLS public keys |
| **VotingPowers** | Source | Converts stake to voting power |
| **Relay Sidecars** | Off-chain | Sign messages with operator BLS keys |
| **Aggregator** | Off-chain | Combines signatures until 2/3+ quorum |
| **Settlement** | Destination | Verifies aggregated BLS proofs |
| **DVN (dst)** | Destination | Bridges Symbiotic proof to LayerZero verify |
| **ReceiveUln302** | Destination | Tracks DVN attestations, commits to Endpoint |

### Contract Deployment

| Contract | Source (31337) | Destination (31338) |
|----------|----------------|---------------------|
| Symbiotic Core | Yes | No |
| Network | Yes | No |
| KeyRegistry | Yes | No |
| VotingPowers | Yes | No |
| Driver | Yes | No |
| Settlement | No | Yes |
| SymbioticLayerZeroDVN | Yes | Yes |
| LayerZero Endpoint | Yes | Yes |
| SendUln302 | Yes | No |
| ReceiveUln302 | No | Yes |

## Quick Start

### Prerequisites

- [Foundry](https://book.getfoundry.sh/getting-started/installation) (forge, anvil, cast)
- [Docker](https://docs.docker.com/get-docker/) (for full devnet)
- [Rust](https://rustup.rs/) (for DVN worker)

### 1. Install Dependencies

```bash
# Install Solidity dependencies
forge install

# Install Node dependencies
bun install
```

### 2. Build Contracts

```bash
forge build
```

### 3. Run Tests

```bash
forge test -vvv
```

### 4. Run Two-Chain Devnet

```bash
# Start devnet (uses existing state or deploys fresh)
./devnet/devnet.sh up

# Force fresh start (wipes all state)
./devnet/devnet.sh up --fresh

# Run E2E test (sends message, verifies delivery)
./devnet/devnet.sh test

# Hot reload contracts after code changes (preserves state)
./devnet/devnet.sh reload dvn
./devnet/devnet.sh reload oapp

# Auto-reload on file changes (requires: brew install watchexec)
./devnet/devnet.sh reload --watch

# Check status
./devnet/devnet.sh status

# View logs
./devnet/devnet.sh logs              # DVN monitor (default)
./devnet/devnet.sh logs deployer     # Deployer

# Stop (state preserved)
./devnet/devnet.sh down

# Stop and wipe all state
./devnet/devnet.sh clean
```

### 5. Chain Endpoints

| Chain | RPC URL | Chain ID |
|-------|---------|----------|
| Source | http://localhost:8545 | 31337 |
| Destination | http://localhost:8546 | 31338 |
| Sidecar 1 | http://localhost:8081 | - |

## Project Structure

```
symbiotic-layerzero-template/
├── src/
│   ├── SymbioticLayerZeroDVN.sol      # Main DVN (deployed on both chains)
│   ├── examples/
│   │   └── TestOApp.sol               # Example OApp for testing
│   └── symbiotic/
│       ├── Settlement.sol              # BLS quorum verification
│       ├── Driver.sol                  # Validator set management
│       ├── KeyRegistry.sol             # Operator key storage
│       └── VotingPowers.sol            # Stake-based voting power
├── script/
│   ├── SourceChainDeploy.s.sol         # Deploy to source chain
│   ├── DestinationChainDeploy.s.sol    # Deploy to destination chain
│   ├── DriverDeploy.s.sol              # Deploy Driver (after Settlements)
│   └── mock/
│       └── MockERC20.sol
├── test/
│   └── SymbioticLayerZeroDVN.t.sol
├── off-chain/
│   └── monitor-operator/
│       ├── Dockerfile                  # Builds OZ Monitor + DVN worker
│       ├── entrypoint.sh               # Dynamic config generation
│       ├── config/
│       │   ├── networks.json           # Chain RPC configs
│       │   ├── monitors.json           # JobAssigned event monitor
│       │   ├── triggers.json           # DVN worker trigger
│       │   └── triggers/scripts/
│       │       └── dvn_worker.sh       # Script wrapper
│       └── workers/layerzero_dvn_worker/
│           └── src/
│               ├── main.rs             # Reads event from stdin
│               ├── sidecar.rs          # Symbiotic Relay gRPC client
│               └── contracts.rs        # Contract bindings
├── devnet/
│   ├── devnet.sh                       # Devnet management script (up/down/status/logs)
│   ├── generate_network.sh             # Generates relay/operator configs
│   ├── docker-compose.yml              # All devnet services
│   └── deploy-data/                    # Generated deployment artifacts
└── README.md
```

## Contracts

### SymbioticLayerZeroDVN

Single contract deployed on both chains with different functions active on each:

**Source Chain Functions:**
| Function | Description |
|----------|-------------|
| `assignJob(param, options)` | Called by SendUln302 to assign verification job |
| `getFee(dstEid, confirmations, sender, options)` | Returns fee for verification |

**Destination Chain Functions:**
| Function | Description |
|----------|-------------|
| `submitVerification(packetHeader, payloadHash, confirmations, epoch, proof)` | Verify Symbiotic proof and notify LayerZero |

### TestOApp (Example)

A simple OApp example is included to demonstrate cross-chain messaging:

```solidity
// Deploy on both chains
TestOApp srcOApp = new TestOApp(endpoint, owner);
TestOApp dstOApp = new TestOApp(endpoint, owner);

// Wire peers
srcOApp.setPeer(dstEid, addressToBytes32(address(dstOApp)));
dstOApp.setPeer(srcEid, addressToBytes32(address(srcOApp)));

// Send a message
srcOApp.ping{value: fee}(dstEid);

// Or with custom message
srcOApp.send{value: fee}(dstEid, message, options);
```

### Message Signing Format

The message signed by Symbiotic validators:

```solidity
// Build deterministic message from LayerZero packet data
bytes32 messageHash = keccak256(abi.encode(
    packetHeader,   // Contains: version, nonce, srcEid, sender, dstEid, receiver
    payloadHash     // Hash of the actual message payload
));

// This is what validators sign via BLS-BN254
bytes memory signedMessage = abi.encode(messageHash);
```

## Off-Chain Operator

### Architecture

The off-chain operator uses [OpenZeppelin Monitor](https://github.com/openzeppelin/openzeppelin-monitor) for event watching and a Rust worker for processing:

```
┌─────────────────────────────────────────────────────────────┐
│                   OpenZeppelin Monitor                       │
│  - Watches JobAssigned events on source chain DVN            │
│  - Polls blocks according to cron schedule                   │
│  - Triggers script when event matches                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ Triggers on JobAssigned event
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    DVN Worker (Rust)                         │
│  1. Receives event data via stdin (JSON)                     │
│  2. Builds message hash from packet data                     │
│  3. Requests BLS signature from Symbiotic Relay sidecar      │
│  4. Waits for aggregation proof (2/3+ quorum)                │
│  5. Submits submitVerification() on destination chain        │
└─────────────────────────────────────────────────────────────┘
```

The worker is invoked by OZ Monitor as a **script trigger**, not as a long-running service.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `SOURCE_RPC_URL` | Yes | Source chain RPC endpoint |
| `DEST_RPC_URL` | Yes | Destination chain RPC endpoint |
| `SOURCE_DVN_ADDRESS` | Yes | DVN contract on source chain |
| `DEST_DVN_ADDRESS` | Yes | DVN contract on destination chain |
| `SIDECAR_URL` | Yes | Symbiotic Relay sidecar URL |
| `PRIVATE_KEY` | Yes | Operator private key |

## Symbiotic Relay Integration

The off-chain node communicates with the Symbiotic Relay via HTTP REST API:

| Endpoint | Description |
|----------|-------------|
| `POST /api/v1/sign_message` | Submit message for BLS signing |
| `POST /api/v1/get_aggregation_proof` | Get aggregated BLS proof |
| `POST /api/v1/get_current_epoch` | Get current epoch |
| `POST /api/v1/get_last_all_committed` | Get committed epochs per chain |

See [Symbiotic Relay HTTP API docs](https://docs.symbiotic.fi/relay-sdk/node/http-api/).

## Configuration

### DVN Contract Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `baseFee` | 0.001 ETH | Fee charged per verification job |
| `JOB_EXPIRY` | 1 hour | Time before pending jobs expire |

### Symbiotic Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `QUORUM_THRESHOLD` | 2/3 + 1 | Minimum voting power for quorum |
| `EPOCH_DURATION` | 60s (devnet) | Duration of each epoch |
| `REQUIRED_KEY_TAG` | 15 | BLS-BN254 key type |

## Deployment

### Three-Phase Deployment

```bash
# Phase 1: Source Chain - Symbiotic infrastructure + DVN
forge script script/SourceChainDeploy.s.sol \
  --rpc-url http://localhost:8545 --broadcast

# Phase 2: Destination Chain - Settlement + DVN
forge script script/DestinationChainDeploy.s.sol \
  --rpc-url http://localhost:8546 --broadcast

# Phase 3: Source Chain - Driver (needs all Settlement addresses)
forge script script/DriverDeploy.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

## License

MIT
