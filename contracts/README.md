# Contracts

Solidity smart contracts for the Symbiotic LayerZero DVN.

## Overview

### SymbioticLayerZeroDVN

The main DVN contract, deployed on both source and destination chains with different active functions.

**Source chain:**
- `assignJob()` - Called by LayerZero's `SendUln302` to request verification
- `getFee()` - Returns the verification fee
- Emits `JobAssigned` events for off-chain operators

**Destination chain:**
- `submitProof()` - Submit Merkle proof with BLS quorum signature
- `addSubmitter()` / `removeSubmitter()` - Manage authorized proof submitters
- `isLeafVerified()` / `isRootVerified()` - Query verification status

**Features:**
- Merkle tree batching for gas-efficient multi-message verification
- Root caching (BLS signature verified once per root, reused for all leaves)
- Authorized submitter whitelist
- Epoch-based signature validity (configurable, default 2 hours)

### Settlement

Symbiotic contract for BLS quorum signature verification against the validator set.

### Supporting Contracts

- `Driver.sol` - Cross-chain message handling
- `KeyRegistry.sol` - Operator key management
- `VotingPowers.sol` - Validator voting power calculations

## Building

```bash
forge build
```

## Testing

```bash
forge test
```

## Deployment

For local development, use `make start` from the repository root.

For manual deployment:

```bash
# Source chain - DVN
forge script script/DeployDVN.s.sol:DeployDVN \
  --sig "deploySource()" \
  --rpc-url $SOURCE_RPC \
  --broadcast \
  --private-key $PRIVATE_KEY

# Destination chain - Settlement
forge script script/DeployDVN.s.sol:DeployDVN \
  --sig "deploySettlement()" \
  --rpc-url $DEST_RPC \
  --broadcast \
  --private-key $PRIVATE_KEY

# Destination chain - DVN (requires Settlement address)
forge script script/DeployDVN.s.sol:DeployDVN \
  --sig "deployDest(address)" $SETTLEMENT_ADDRESS \
  --rpc-url $DEST_RPC \
  --broadcast \
  --private-key $PRIVATE_KEY

# Relay infrastructure
forge script script/DeployRelayInfra.s.sol:DeployRelayInfra \
  --rpc-url $SOURCE_RPC \
  --broadcast \
  --private-key $PRIVATE_KEY
```

## Output

Deployment scripts write contract addresses to `deploy-data/`:

- `source_contracts.json` - DVN and SendUln on source chain
- `dest_contracts.json` - DVN, ReceiveUln, and Settlement on destination chain
- `relay_infra.json` - Symbiotic relay infrastructure (Settlement, KeyRegistry, VotingPowers, Driver)
