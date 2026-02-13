# Manual Testing Guide

Test provider-aware flows (`layerzero` and `chainlink_ccv`) with simple commands.

## Quick Start

```bash
# First time setup
make setup

# Select provider in config/root.config.json:
#   "active_provider": "layerzero" | "chainlink_ccv"
make start

# Run full E2E test
make e2e
```

## Commands

### Send a Message

```bash
make send MSG="hello world"
```

<details>
<summary>Underlying commands</summary>

```bash
# LayerZero path (example)
# Build executor options (200k gas for lzReceive)
OPTIONS=$(cast call $TEST_OAPP_SOURCE_ADDRESS \
  "buildOptions(uint128)(bytes)" 200000 \
  --rpc-url http://localhost:8545)

# Quote the messaging fee
QUOTE=$(cast call $TEST_OAPP_SOURCE_ADDRESS \
  "quote(uint32,string,bytes,bool)((uint256,uint256))" \
  31338 "hello world" "$OPTIONS" false \
  --rpc-url http://localhost:8545)

# Send the message
cast send $TEST_OAPP_SOURCE_ADDRESS \
  "send(uint32,string,bytes)" \
  31338 "hello world" "$OPTIONS" \
  --value 0.01ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545

# Find message GUID from operator
curl -s http://localhost:3001/debug/v1/messages | \
  jq '.messages[0].metadata.message_id'
```
</details>

`make send` is provider-aware:
- `layerzero`: sends through `TestOApp.send(...)`
- `chainlink_ccv`: sends through source mock `OnRamp.sendMessage(...)` and emits `CCIPMessageSent`

---

### Watch Message Lifecycle

```bash
make watch
```

<details>
<summary>Underlying commands</summary>

```bash
# Poll operator for message status
curl -s http://localhost:3001/debug/v1/messages | \
  jq '.messages[] | select(.metadata.message_id == "<guid>") | {status, submission}'

# LayerZero destination verification
cast logs --from-block 0 --address $DVN_DEST_ADDRESS \
  --rpc-url http://localhost:8546
```
</details>

`make watch` is provider-aware:
- `layerzero`: success when destination target verification is observed on-chain
- `chainlink_ccv`: success when destination `MessageExecuted(messageId)` is observed on-chain

Options:
- `GUID=0x...` - Watch specific message by GUID
- `TX=0x...` - Watch message by source TX hash
- `TIMEOUT=120` - Max wait time in seconds

---

### Check Status

```bash
make status-msg
```

<details>
<summary>Underlying commands</summary>

```bash
# Query all operators
for port in 3001 3002 3003; do
  curl -s "http://localhost:$port/debug/v1/messages?limit=1" | \
    jq '.messages[0] | {status, submission}'
done

# Check pending merkle roots
curl -s http://localhost:3001/debug/v1/pending
```
</details>

---

### Full E2E Test

```bash
make e2e
```

Combines `send` + `watch` into one command. Shows timeline:

```
[18:53:21] Operators: waiting to batch
[18:53:28] Operators: collecting BLS signatures
[18:53:30] Operators: signed (quorum reached)
[18:53:32] Relayer: submitted
[18:53:34] Relayer: confirmed (tx: 0x4617...)
[18:53:34] Destination target: verified on-chain (tx: 0x4617...)

Message verified on destination chain!
```

Options:
- `MSG="..."` - Custom message
- `TIMEOUT=120` - Max wait time
- `VERBOSE=1` - Show streaming logs

---

## Using the CLI Directly

The `scripts/msg` tool provides direct access:

```bash
./scripts/msg send --message "test"
./scripts/msg status
./scripts/msg watch --timeout 60
./scripts/msg e2e --verbose

# Show underlying commands without executing
./scripts/msg send --dry-run
./scripts/msg e2e --dry-run
```

---

## Message Lifecycle

| Stage | Component | Status | What's Happening |
|-------|-----------|--------|------------------|
| 1 | Operators | Pending | Received via webhook, awaiting batching |
| 2 | Operators | Processing | Batched into merkle tree, awaiting BLS signatures |
| 3 | Operators | Signed | Quorum signatures collected |
| 4 | Relayer | Submitted | Proof sent to OZ Relayer |
| 5 | Relayer | Confirmed | On-chain TX confirmed via webhook |
| 6 | Destination | Verified | Provider-specific destination verification observed |

---

## Troubleshooting

### Message not appearing

```bash
# Check OZ Monitor detected the event
docker logs oz-monitor --tail 50 | grep -Ei "jobassigned|ccipmessagesent"

# Check webhook delivery to operators
docker logs operator-1 --tail 50 | grep -i webhook
```

### Stuck at "Pending"

```bash
# Check operator batching logs
docker logs operator-1 --tail 100 | grep -i "batch\|merkle"
```

### Stuck at "Processing"

```bash
# Check sidecar health
curl -s http://localhost:8081/healthz

# Check sidecar logs
docker logs symbiotic-relay-1 --tail 50
```

### Stuck at "Signed"

```bash
# Check relayer health
curl -s http://localhost:8080/api/v1/health

# Check relayer logs
docker logs oz-relayer --tail 100
```

---

## Service Ports

| Service | Port | Purpose |
|---------|------|---------|
| anvil (source) | 8545 | Source chain RPC |
| anvil (dest) | 8546 | Destination chain RPC |
| operator-1/2/3 | 3001-3003 | Operator debug APIs |
| symbiotic-relay-1/2/3 | 8081-8083 | BLS sidecars |
| oz-relayer | 8080 | Transaction relayer |

---

## Debug API Reference

### GET /debug/v1/messages

```bash
curl -s http://localhost:3001/debug/v1/messages | jq '.messages'
```

Query params: `status=pending|processing|signed`, `limit=50`, `offset=0`

### GET /debug/v1/pending

```bash
curl -s http://localhost:3001/debug/v1/pending
```

Returns merkle roots awaiting BLS signatures. Empty = all collected.
