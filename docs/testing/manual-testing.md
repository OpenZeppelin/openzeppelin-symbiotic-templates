# Manual Testing Guide

This guide covers testing the Symbiotic LayerZero DVN, from quick commands to detailed manual inspection.

## Quick Start

The fastest way to test end-to-end:

```bash
# Send a test message
make send MSG="hello world"

# Watch it progress through the pipeline
make watch
```

The `watch` command shows live status as your message moves through:
- Operators receive the event
- BLS signatures are collected
- Proof is submitted to destination chain
- DVN verifies the message

## Prerequisites

```bash
# First time setup
make setup

# Start all services (deploys contracts, generates configs)
make start

# Verify services are healthy
make status
```

## Testing Commands Reference

| Command | Description |
|---------|-------------|
| `make send MSG="..."` | Send a test message |
| `make watch` | Watch last message's lifecycle |
| `make watch GUID=0x...` | Watch specific message by GUID |
| `make watch TX=0x...` | Watch message by source TX hash |
| `make msg-status` | Quick status check across all operators |
| `make test` | Run automated E2E test |
| `make shell` | Interactive shell with addresses loaded |

---

## Detailed Manual Testing

For full control, you can run each step manually.

### Step 1: Load Contract Addresses

```bash
# Option A: Use the interactive shell (recommended)
make shell

# Option B: Source the addresses file
source data/deploy-data/addresses.env

# Verify addresses are loaded
echo "DVN Source: $DVN_SOURCE_ADDRESS"
echo "DVN Dest:   $DVN_DEST_ADDRESS"
echo "TestOApp:   $TEST_OAPP_SOURCE_ADDRESS"
```

### Step 2: Send a Cross-Chain Message

```bash
# Set private key (Anvil account 0)
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

# Build executor options (200k gas for lzReceive on dest)
OPTIONS=$(cast call "$TEST_OAPP_SOURCE_ADDRESS" \
  "buildOptions(uint128)(bytes)" 200000 \
  --rpc-url $SOURCE_RPC_URL)

echo "Options: $OPTIONS"

# Quote the messaging fee
QUOTE=$(cast call "$TEST_OAPP_SOURCE_ADDRESS" \
  "quote(uint32,string,bytes,bool)((uint256,uint256))" \
  $DEST_CHAIN_ID "hello" "$OPTIONS" false \
  --rpc-url $SOURCE_RPC_URL)

echo "Quote: $QUOTE"

# Send the message
TX=$(cast send "$TEST_OAPP_SOURCE_ADDRESS" \
  "send(uint32,string,bytes)" \
  $DEST_CHAIN_ID "hello" "$OPTIONS" \
  --value 0.01ether \
  --private-key $PRIVATE_KEY \
  --rpc-url $SOURCE_RPC_URL \
  --json | jq -r '.transactionHash')

echo "TX Hash: $TX"

# Verify message count incremented
cast call "$TEST_OAPP_SOURCE_ADDRESS" "messagesSent()(uint256)" --rpc-url $SOURCE_RPC_URL
```

### Step 3: Check DVN Events on Source Chain

The DVN emits `JobAssigned` when it receives a verification job.

```bash
# Get all DVN events
cast logs --from-block 0 --address "$DVN_SOURCE_ADDRESS" --rpc-url $SOURCE_RPC_URL

# Decode the JobAssigned event (topic0 is the event signature)
TOPIC0=$(cast keccak "JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)")
cast logs --from-block 0 --address "$DVN_SOURCE_ADDRESS" \
  --topic0 "$TOPIC0" \
  --rpc-url $SOURCE_RPC_URL

# Get just the transaction hashes
cast logs --from-block 0 --address "$DVN_SOURCE_ADDRESS" \
  --rpc-url $SOURCE_RPC_URL | grep transactionHash
```

### Step 4: Query Operator Debug APIs

Each operator exposes debug endpoints on ports 3001-3003.

```bash
# List all messages on operator-1
curl -s http://localhost:3001/debug/v1/messages | jq '.messages'

# Get message count
curl -s http://localhost:3001/debug/v1/messages | jq '.count'

# Filter by status (pending, processing, signed)
curl -s "http://localhost:3001/debug/v1/messages?status=signed" | jq '.messages'

# Find message by TX hash
TX="0x..."  # Your source chain TX
curl -s http://localhost:3001/debug/v1/messages | \
  jq --arg tx "$TX" '.messages[] | select(.metadata.event_tx_hash == $tx)'

# Find message by GUID
GUID="0x..."
curl -s http://localhost:3001/debug/v1/messages | \
  jq --arg id "$GUID" '.messages[] | select(.metadata.message_id == $id)'

# Check pending merkle roots (awaiting BLS signatures)
curl -s http://localhost:3001/debug/v1/pending | jq '.'

# Compare status across all operators
for port in 3001 3002 3003; do
  echo "=== Operator on port $port ==="
  curl -s "http://localhost:$port/debug/v1/messages?limit=1" | \
    jq '.messages[0] | {status, submission}'
done
```

### Step 5: Check DVN Verification on Destination Chain

```bash
# Watch for VerificationSubmitted events
cast logs --from-block 0 --address "$DVN_DEST_ADDRESS" --rpc-url $DEST_RPC_URL

# Get the event topic for filtering
TOPIC0=$(cast keccak "VerificationSubmitted(bytes32,bytes32,uint64)")
cast logs --from-block 0 --address "$DVN_DEST_ADDRESS" \
  --topic0 "$TOPIC0" \
  --rpc-url $DEST_RPC_URL

# Check if a specific leaf was verified
LEAF_HASH="0x..."  # From VerificationSubmitted event data
cast call "$DVN_DEST_ADDRESS" "isLeafVerified(bytes32)(bool)" "$LEAF_HASH" \
  --rpc-url $DEST_RPC_URL
```

### Step 6: Inspect the Proof

```bash
# Get proof for a message via operator API
GUID="0x..."
curl -s http://localhost:3001/api/v1/layerzero/proof \
  -H "Content-Type: application/json" \
  -d "{\"message_ids\": [\"$GUID\"]}" | jq '.'

# Verify a proof locally (before on-chain submission)
curl -s http://localhost:3001/api/v1/layerzero/verify \
  -H "Content-Type: application/json" \
  -d '{"root_hash": "0x...", "leaf": "0x...", ...}'
```

---

## Debug API Reference

### GET /debug/v1/messages

List all messages with processing and submission status.

**Query Parameters:**
- `status` - Filter by: `pending`, `processing`, `signed`
- `limit` - Max results (default: 50)
- `offset` - Skip N results

**Response:**
```json
{
  "messages": [
    {
      "metadata": {
        "source_chain": 31337,
        "destination_chain": 31338,
        "message_id": "0xabcd...",
        "event_tx_hash": "0x1234..."
      },
      "status": "Signed",
      "submission": {
        "state": "Confirmed",
        "tx_hash": "0x5678...",
        "relayer_tx_id": "tx-123"
      }
    }
  ],
  "count": 1,
  "limit": 50,
  "offset": 0
}
```

### GET /debug/v1/pending

List merkle roots awaiting BLS signatures.

```json
["0xroot1...", "0xroot2..."]
```

Empty array means all signatures have been collected.

### Message Status Lifecycle

| Status | Description |
|--------|-------------|
| `Pending` | Received via webhook, awaiting batching |
| `Processing` | Batched into merkle tree, awaiting BLS signatures |
| `Signed` | Quorum signatures collected, ready for submission |

### Submission Status Lifecycle

| State | Description |
|-------|-------------|
| `Pending` | Not yet submitted to relayer |
| `Submitted` | Sent to OZ Relayer, `relayer_tx_id` available |
| `Confirmed` | On-chain TX confirmed, `tx_hash` available |
| `Failed` | Submission failed |

---

## Troubleshooting

### Message not appearing in operators

**Symptom:** `make msg-status` shows no messages

**Check OZ Monitor:**
```bash
docker logs oz-monitor --tail 100 | grep -i "jobassigned\|error"
```

**Check webhook delivery:**
```bash
docker logs operator-1 --tail 50 | grep -i webhook
```

### Message stuck at "Pending"

**Symptom:** Status stays `Pending`, never moves to `Processing`

The signer batch interval may not have triggered yet, or there's an issue with the batching logic.

```bash
# Check operator logs for batching
docker logs operator-1 --tail 100 | grep -i "batch\|merkle"
```

### Message stuck at "Processing"

**Symptom:** Status is `Processing` but never becomes `Signed`

BLS signatures aren't being collected from sidecars.

```bash
# Check sidecar health
curl -s http://localhost:8081/healthz
curl -s http://localhost:8082/healthz
curl -s http://localhost:8083/healthz

# Check sidecar logs
docker logs symbiotic-relay-1 --tail 50
```

### Message stuck at "Signed" / Submission Pending

**Symptom:** Status is `Signed` but `submission.state` stays `Pending`

OZ Relayer isn't picking up the proof submission request.

```bash
# Check relayer health
curl -s http://localhost:8080/api/v1/health \
  -H "Authorization: Bearer $OZ_RELAYER_API_KEY"

# Check relayer logs
docker logs oz-relayer --tail 100
```

### DVN verification failed

**Symptom:** TX submitted but no `VerificationSubmitted` event

```bash
# Check for reverts in relayer logs
docker logs oz-relayer --tail 200 | grep -i "revert\|error\|fail"

# Check the dest chain for failed TXs
cast logs --from-block 0 --address "$DVN_DEST_ADDRESS" --rpc-url $DEST_RPC_URL
```

---

## Service Ports Reference

| Service | Port | Purpose |
|---------|------|---------|
| anvil (source) | 8545 | Source chain RPC (chain ID: 31337) |
| anvil (dest) | 8546 | Dest chain RPC (chain ID: 31338) |
| operator-1 | 3001 | Operator API |
| operator-2 | 3002 | Operator API |
| operator-3 | 3003 | Operator API |
| symbiotic-relay-1 | 8081 | BLS sidecar |
| symbiotic-relay-2 | 8082 | BLS sidecar |
| symbiotic-relay-3 | 8083 | BLS sidecar |
| oz-relayer | 8080 | Transaction relayer |
| redis | 6379 | Job queue |
