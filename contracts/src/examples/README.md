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

### Usage

#### 1. Deploy TestOApp on both chains

```bash
# Source chain
forge script DeployTestOApp --sig "deploySource(address)" <endpoint> \
  --rpc-url http://localhost:8545 --broadcast

# Destination chain
forge script DeployTestOApp --sig "deployDest(address)" <endpoint> \
  --rpc-url http://localhost:8546 --broadcast
```

#### 2. Configure peers

```bash
# On source chain
forge script DeployTestOApp --sig "configurePeers(address,address)" <srcOApp> <dstOApp> \
  --rpc-url http://localhost:8545 --broadcast

# On destination chain
forge script DeployTestOApp --sig "configurePeers(address,address)" <dstOApp> <srcOApp> \
  --rpc-url http://localhost:8546 --broadcast
```

#### 3. Send a test message

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
forge test --match-path test/examples/TestOApp.t.sol -vvv
```

## Dependencies

The example contracts require:

- `@layerzerolabs/oapp-evm` - LayerZero OApp base contracts
- `@layerzerolabs/lz-evm-protocol-v2` - LayerZero protocol interfaces
- `@openzeppelin/contracts` - OpenZeppelin utilities
