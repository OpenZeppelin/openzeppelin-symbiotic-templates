# Example Contracts

This directory contains example contracts demonstrating LayerZero cross-chain messaging with the Symbiotic DVN.

> **WARNING**: These contracts are for testing and demonstration purposes only. **DELETE THIS DIRECTORY** before deploying to production.

## TestOApp

A simple OApp (Omnichain Application) that demonstrates:

- Sending cross-chain messages via LayerZero
- Receiving and processing messages from other chains
- Quoting fees for cross-chain transactions
- Using the OptionsBuilder for execution options

### Message Flow

```
Source Chain                          Destination Chain
============                          =================

1. User calls send()
   ↓
2. TestOApp encodes message
   ↓
3. _lzSend() to endpoint
   ↓
4. Endpoint → SendUln302
   ↓
5. SendUln302 → DVN.assignJob()
   ↓
6. [Off-chain: Symbiotic operators]
   - Monitor JobAssigned events
   - Batch and sign with BLS keys
   - Relay sidecar aggregates signatures
   ↓                                  7. Relayer → DVN.submitProof()
                                         ↓
                                      8. DVN verifies BLS quorum
                                         ↓
                                      9. DVN → ReceiveUln302.verify()
                                         ↓
                                      10. Executor → TestOApp._lzReceive()
                                         ↓
                                      11. Message stored, event emitted
```

### Quick Start (Automated)

The easiest way to test TestOApp is with the automated E2E test:

```bash
# From the repository root
make setup  # Generate keys (first time only)
make start  # Deploys everything including TestOApp
make test   # Runs E2E test with TestOApp
```

The `make start` command handles deployment in 4 phases:
1. Core contracts (DVN, Settlement)
2. Symbiotic relay infrastructure
3. Operator registration
4. **TestOApp deployment and peer configuration**

After `make test`, you can check the debug API to see message status:

```bash
# List all messages
curl http://localhost:3001/debug/v1/messages | jq

# Filter by status
curl "http://localhost:3001/debug/v1/messages?status=signed" | jq
```

### Manual Usage

#### 1. Deploy the LayerZero stack on both chains

```bash
# Local devnet
forge script script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack \
  --sig "deployLocal()" \
  --broadcast \
  --multi \
  --private-key $PRIVATE_KEY

# External networks
forge script script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack \
  --sig "deployExternal()" \
  --broadcast \
  --multi \
  --private-key $PRIVATE_KEY
```

#### 2. Send a test message

```bash
forge script SendTestMessage --sig "run(address,uint32,string)" \
  <testOApp> 31338 "Hello from source!" \
  --rpc-url http://localhost:8545 --broadcast
```

### Contract Interface

```solidity
// Send a message to another chain
function send(
    uint32 _dstEid,      // Destination endpoint ID
    string calldata _message,
    bytes calldata _options
) external payable returns (MessagingReceipt memory);

// Quote the fee for sending a message
function quote(
    uint32 _dstEid,
    string calldata _message,
    bytes calldata _options,
    bool _payInLzToken
) external view returns (MessagingFee memory);

// Build execution options (convenience function)
function buildOptions(uint128 _gas) external pure returns (bytes memory);
```

### State Variables

- `lastMessage` - The last message received
- `lastSrcEid` - Source endpoint ID of last message
- `lastSender` - Sender address of last message
- `messagesSent` - Counter for sent messages
- `messagesReceived` - Counter for received messages

### Events

```solidity
event MessageSent(uint32 indexed dstEid, string message, bytes32 guid, uint64 nonce);
event MessageReceived(uint32 indexed srcEid, bytes32 sender, string message, bytes32 guid);
```

## Testing

Run the example tests:

```bash
cd contracts

# Unit tests (mock environment)
forge test --match-path test/examples/TestOApp.t.sol -vvv

# Integration tests (full LayerZero stack)
forge test --match-path test/examples/TestOAppIntegration.t.sol -vvv
```

## Troubleshooting

### Message Not Received on Destination

1. Check if the message was processed by operators:
   ```bash
   curl "http://localhost:3001/debug/v1/messages?status=signed" | jq
   ```

2. Verify the DVN proof was submitted:
   ```bash
   curl http://localhost:3001/debug/v1/messages | jq '.[].submission'
   ```

3. Check operator logs for errors:
   ```bash
   make logs-operators | grep -i error
   ```

### JobAssigned Event Not Detected

1. Verify OZ Monitor is running and watching the correct contract:
   ```bash
   make logs-monitor | grep JobAssigned
   ```

2. Check webhook delivery:
   ```bash
   make logs-operators | grep webhook
   ```

### Insufficient Fee Error

When sending messages, ensure you provide enough ETH for the LayerZero fee:
```bash
# Quote the fee first
cast call <testOApp> "quote(uint32,string,bytes,bool)" 31338 "Hello" "0x" false
```

## Dependencies

The example contracts require:

- `@layerzerolabs/oapp-evm` - LayerZero OApp base contracts
- `@layerzerolabs/lz-evm-protocol-v2` - LayerZero protocol interfaces
- `@openzeppelin/contracts` - OpenZeppelin utilities
