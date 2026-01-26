# Security Model

Security architecture and trust assumptions for SymbioticLayerZeroDVN.

## Trust Assumptions

| Entity | Trust Level | Notes |
|--------|-------------|-------|
| **SendUln302** | Trusted | LayerZero's send library; only caller for `assignJob` |
| **Settlement** | Trusted | Symbiotic contract for BLS signature verification |
| **Authorized Submitters** | Semi-trusted | Whitelisted addresses that submit proofs; cannot forge signatures but can grief (spam invalid proofs) |
| **Owner** | Trusted | Admin with pause/unpause, submitter management, fee withdrawal |
| **External users** | Untrusted | Cannot call privileged functions directly |

## Access Control

### Source Chain Functions

| Function | Caller | Purpose |
|----------|--------|---------|
| `assignJob` | SendUln302 only | Register verification job, emit event |
| `getFee` | Anyone | Query verification fee (view) |

### Destination Chain Functions

| Function | Caller | Purpose |
|----------|--------|---------|
| `submitProof` | Authorized submitters | Submit signed Merkle proof for verification |

### Admin Functions

| Function | Caller | Purpose |
|----------|--------|---------|
| `addSubmitter` | Owner | Whitelist a submitter address |
| `removeSubmitter` | Owner | Remove submitter from whitelist |
| `setBaseFee` | Owner | Update verification fee |
| `pause` | Owner | Emergency pause all operations |
| `unpause` | Owner | Resume operations |
| `withdraw` | Owner | Recover ETH (force-sent or accidental) |
| `transferOwnership` | Owner | Transfer admin rights |

### View Functions

| Function | Caller | Purpose |
|----------|--------|---------|
| `isSubmitter` | Anyone | Check if address is authorized |
| `isLeafVerified` | Anyone | Check if leaf was verified |
| `isRootVerified` | Anyone | Check if Merkle root is cached |
| `computeLeaf` | Anyone | Compute leaf hash for given inputs |
| `verifyMerkleProof` | Anyone | Verify proof off-chain |

## Invariants

Properties that must always hold:

1. **Leaf monotonicity**: `verifiedLeaves[leaf]` can only transition `false → true`, never back
2. **Root monotonicity**: `verifiedRoots[root]` can only transition `false → true`, never back
3. **Signature requirement**: Uncached roots require valid BLS quorum signature from Settlement
4. **Packet header integrity**: All verified packets have exactly 81 bytes and correct `dstEid`
5. **No ETH custody**: Contract does not collect fees; `assignJob` rejects `msg.value > 0`

## Deployment Modes

The contract supports three deployment configurations:

| Mode | sendUln | receiveUln | settlement | Use case |
|------|---------|------------|------------|----------|
| Source only | Set | Zero | Zero | Emit `JobAssigned` events |
| Destination only | Zero | Set | Set | Verify proofs, call ReceiveUln |
| Bidirectional | Set | Set | Set | Both functions on same chain |

## External Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `@openzeppelin/contracts` | 5.x | MerkleProof verification |
| `@symbioticfi/relay-contracts` | - | Settlement base contracts |
| LayerZero V2 | - | ILayerZeroDVN interface |

## Security Considerations

### What the contract does NOT do

- **Fee custody**: Fees are handled by LayerZero's fee accounting, not this contract
- **Signature generation**: BLS signing happens off-chain via Symbiotic Relay
- **Slashing**: Handled by Symbiotic core contracts, not this DVN
