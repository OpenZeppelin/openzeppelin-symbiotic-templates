# API Reference

Each operator exposes HTTP endpoints for webhooks and debugging.

## Webhook Endpoints

### POST /webhook/events

Receives provider ingress events from OZ Monitor.

Ingress event by active provider:
- `layerzero`: `JobAssigned`
- `chainlink_ccv`: `CCIPMessageSent`

**Authentication:** HMAC-SHA256 signature verification using two headers:
- `X-Signature`: Hex-encoded HMAC-SHA256 of `body + timestamp`
- `X-Timestamp`: Unix timestamp in milliseconds

The webhook uses OZ Monitor's native webhook trigger with `payload_mode: "raw"`. The trigger template is at `config/templates/oz-monitor/triggers/webhook_layerzero.json`. The webhook secret must match between:
- Operator: `WEBHOOK_SECRET` environment variable (min 32 chars)
- OZ Monitor: `config.secret.value` in the trigger configuration

### POST /api/v1/webhooks/oz-relayer

Receives transaction status updates from OZ Relayer for submission tracking.

**Authentication:** HMAC-SHA256 signature in the `X-Signature` header using `OZ_RELAYER_WEBHOOK_SECRET`. The signature is Base64-encoded HMAC-SHA256 of the raw JSON request body.

## Debug Endpoints

### GET /debug/v1/messages

List all messages with their processing and submission status.

**Query Parameters:**

| Parameter | Default | Description |
|-----------|---------|-------------|
| `status` | (all) | Filter by processing status: `pending`, `processing`, or `signed` |
| `limit` | 50 | Maximum number of messages to return |
| `offset` | 0 | Number of messages to skip (for pagination) |

**Response:**
```json
{
  "messages": [
    {
      "metadata": {
        "source_chain": 31337,
        "destination_chain": 31338,
        "block_number": 123,
        "message_id": "0xabc...",
        "event_tx_hash": "0xdef..."
      },
      "data": "...",
      "status": "Signed",
      "submission": {
        "state": "Confirmed",
        "tx_hash": "0x...",
        "relayer_tx_id": "tx-123"
      }
    }
  ],
  "count": 1,
  "limit": 50,
  "offset": 0
}
```

### GET /debug/v1/messages/:message_id

Get a specific message by ID.

**Response:** Same format as a single message in the list endpoint.

### GET /debug/v1/pending

List Merkle roots awaiting BLS signatures.

## Proof Endpoints

### POST /api/v1/layerzero/proof

Retrieve Merkle proofs for messages that have been processed into a tree.

**Request:**
```json
{
  "message_ids": ["0xabc123...", "0xdef456..."]
}
```

**Response:** Map of message ID to proof (messages not found are omitted):
```json
{
  "0xabc123...": {
    "root_hash": "0x...",
    "root_proof": [],
    "index": 2,
    "leaf": "0x...",
    "siblings": ["0x...", "0x..."],
    "original_list": ["0x...", "0x..."]
  }
}
```

| Field | Description |
|-------|-------------|
| `root_hash` | Merkle root containing this message |
| `root_proof` | BLS aggregation signature (empty until signed) |
| `index` | Bit-encoded path in the tree |
| `leaf` | DVN-compatible leaf hash |
| `siblings` | Sibling hashes for proof verification |
| `original_list` | All leaf hashes in the tree |

### POST /api/v1/layerzero/verify

Verify a Merkle proof is valid (useful for testing before on-chain submission).

**Request:** A proof object (as returned by `/proof` endpoint)

**Response:** `"valid"` or `"invalid"`

## Message Status Lifecycle

| Status | Description |
|--------|-------------|
| `Pending` | Message received via webhook, awaiting batching |
| `Processing` | Message batched into a Merkle tree, awaiting BLS signatures |
| `Signed` | Merkle root signed by BLS quorum, ready for submission |

## Submission Status

| State | Description |
|-------|-------------|
| `Pending` | Awaiting submission to relayer |
| `Submitted` | Sent to OZ Relayer, `relayer_tx_id` available |
| `Confirmed` | On-chain transaction confirmed, `tx_hash` available |
| `Failed` | Submission failed (check operator logs) |

## Health Check

### GET /healthz

Returns service health status.

**Response:** `200 OK` if healthy.
