# Configuration

This guide covers configuration for the Symbiotic multi-provider template.

## Config Structure

The devnet uses a **template-based configuration** system to keep the git working tree clean:

```
config/
├── templates/              # Source templates (tracked in git)
│   ├── operator/
│   │   └── config.json     # Operator config template
│   └── oz-monitor/
│       ├── monitors/       # Monitor definitions
│       ├── networks/       # Network configs
│       └── triggers/       # Webhook triggers
├── oz-relayer/             # Static configs (no patching needed)
└── symbiotic-relay/        # Static configs

data/
├── generated-config/       # Runtime configs (gitignored, generated)
│   ├── operator-1/
│   ├── operator-2/
│   ├── operator-3/
│   └── oz-monitor/
└── deploy-data/            # Deployment artifacts
    └── addresses.env       # All addresses (shell-sourceable)
```

**How it works:**
1. `make start` deploys contracts and runs `make configure`
2. `make configure` reads templates, patches in deployed addresses, writes to `data/generated-config/`
3. Docker containers mount from `data/generated-config/`

**To customize configs:** Edit the templates in `config/templates/`, then run `make configure` to regenerate.

## Root Provider Config

`config/root.config.json` is the control-plane config (provider selection + provider selectors).

Runtime addresses do **not** live in root config:
1. discovered addresses come from `data/deploy-data/*.json`
2. optional ad-hoc overrides come from `CCV_*` environment variables
3. generated runtime configs in `data/generated-config/` are derived artifacts

For this repo:
1. Exactly one provider is active at a time.
2. `make start`, `make send`, and `make watch` are provider-aware based on `active_provider`.

Example:

```json
{
  "active_provider": "chainlink_ccv",
  "providers": {
    "layerzero": {
      "source_chain_id": 31337,
      "destination_chain_id": 31338,
      "source_eid": 31337,
      "destination_eid": 31338
    },
    "chainlink_ccv": {
      "source_chain_selector": 31337,
      "destination_chain_selector": 31338
    }
  }
}
```

LayerZero config contract:
1. `providers.layerzero.{source_chain_id,destination_chain_id,source_eid,destination_eid}` are required.
2. `make configure` uses these values to generate `destination_chains`, `chain_relayers[*].chain_id`, and `layerzero.eid_to_chain_id`.
3. `make start` passes `source_eid`/`destination_eid` into LayerZero contract deployment and ULN configuration scripts.
4. `make configure` fails if root LayerZero chain IDs/EIDs drift from deploy artifacts (`source_contracts.json`, `dest_contracts.json`, `layerzero_{source,dest}.json`).

Address resolution precedence for CCV scripts is:
1. `CCV_*` env vars
2. deploy artifacts in `data/deploy-data/ccv_{source,dest}_contracts.json`

CCV settlement addresses are sourced from relay infra deploy artifacts:
1. source chain: `data/deploy-data/relay_infra_source.json`
2. destination chain: `data/deploy-data/relay_infra.json`

## Environment Variables

`make start` automatically runs environment bootstrap (`make ensure-env`) and will create `.env`/keystores when missing.

Use `make setup` only when you want to explicitly regenerate local `.env` and keys, or copy from `.env.example` manually:

| Variable                    | Description                                           |
| --------------------------- | ----------------------------------------------------- |
| `PRIVATE_KEY`               | Deployer key (default: Anvil account 0)               |
| `LOG_LEVEL`                 | Logging level (debug, info, warn, error)              |
| `WEBHOOK_SECRET`            | HMAC secret for webhook authentication (min 32 chars) |
| `OZ_RELAYER_WEBHOOK_SECRET` | Secret for OZ Relayer webhook auth (min 32 chars)     |
| `OZ_RELAYER_API_KEY`        | **Required.** Relayer API authentication              |
| `SIDECAR_*_SECRET_KEYS`     | BLS keys per operator (generated)                     |

### Security

The operator requires webhook secrets for secure communication. Generate secrets with:

```bash
openssl rand -hex 32
```

**Required environment variables:**

- `WEBHOOK_SECRET` - HMAC secret for `/webhook/events` endpoint (min 32 chars)
- `OZ_RELAYER_WEBHOOK_SECRET` - HMAC secret for `/api/v1/webhooks/oz-relayer` endpoint (min 32 chars)
- `OZ_RELAYER_API_KEY` - API key for OZ Relayer authentication

The operator will fail to start if these are missing. Secrets must be at least 32 characters.

## Operator Configuration

Operator configs are generated from templates at startup. The source template is `config/templates/operator/config.json`, and runtime configs are written to `data/generated-config/operator-{n}/config.json`.

To regenerate configs after changing templates:

```bash
make configure
```

Key settings:

| Section | Setting | Default | Description |
|---------|---------|---------|-------------|
| `symbiotic_relay` | `address` | - | BLS sidecar gRPC endpoint |
| `symbiotic_relay` | `max_retries` | 3 | Retry attempts for gRPC calls |
| `symbiotic_relay` | `retry_backoff` | 1s | Base backoff (linear: backoff × attempt) |
| `oz_relayer` | `base_url` | - | OZ Relayer HTTP endpoint |
| `oz_relayer` | `max_retries` | 3 | Retry attempts for HTTP calls |
| `oz_relayer` | `retry_backoff` | 1s | Base backoff (exponential: backoff × 2^attempt) |
| `oz_relayer` | `chain_relayers` | - | Per-chain relayer configuration |
| - | `destination_chains` | - | Chain IDs to submit proofs to |

### Example config.json

```json
{
  "symbiotic_relay": {
    "address": "http://symbiotic-relay-1:8080",
    "max_retries": 3,
    "retry_backoff": "1s"
  },
  "oz_relayer": {
    "base_url": "http://oz-relayer:8080",
    "max_retries": 3,
    "retry_backoff": "1s",
    "timeout": "30s"
  }
}
```

> **Note:** The symbiotic-relay containers use port 8080 internally. The host-mapped ports (8081-8083) are only for external access outside Docker.

| Setting | Description | Default |
|---------|-------------|---------|
| `max_retries` | Maximum retry attempts (0 = no retries) | 3 |
| `retry_backoff` | Base backoff duration | 1s |
| `timeout` | Request timeout (OZ Relayer only) | 30s |

## Retry Configuration

The operator uses different retry strategies for its external service calls.

### Symbiotic Relay (Linear Backoff)

Used for gRPC calls to the BLS signing sidecar.

**Formula:**
```
backoff = retry_backoff × (attempt + 1)
```

**Example** with `retry_backoff: 1s` and `max_retries: 3`:
| Attempt | Wait Time |
|---------|-----------|
| 1 | Immediate |
| 2 | 1 second |
| 3 | 2 seconds |
| 4 | 3 seconds |

**Total maximum wait:** ~6 seconds

### OZ Relayer (Exponential Backoff with Jitter)

Used for HTTP calls to the transaction relayer.

**Formula:**
```
base = retry_backoff × 2^attempt
jitter = random(0, base × 0.25)
backoff = min(base + jitter, 60s)
```

**Example** with `retry_backoff: 1s` and `max_retries: 3`:
| Attempt | Base | Jitter Range | Actual Wait |
|---------|------|--------------|-------------|
| 1 | Immediate | - | 0 |
| 2 | 1s | 0-250ms | ~1-1.25s |
| 3 | 2s | 0-500ms | ~2-2.5s |
| 4 | 4s | 0-1s | ~4-5s |

**Maximum single backoff:** 60 seconds (hard cap)

**Why jitter?** Prevents the "thundering herd" problem when multiple operators retry simultaneously after a shared failure.

### Retryable vs Non-Retryable Errors

**Retryable errors** (will be retried):
- HTTP 429 (Too Many Requests / Rate Limit)
- HTTP 500-504 (Server errors)
- Network errors (connection refused, timeout, DNS failure)

**Non-retryable errors** (fail immediately):
- HTTP 4xx (except 429)
- Domain errors (chain not configured, transaction not found)

### Tuning Guidelines

**Low-Latency Environments (Devnet/Testnet):**
```json
{
  "oz_relayer": {
    "max_retries": 5,
    "retry_backoff": "100ms"
  }
}
```

**Production Environments:**
```json
{
  "oz_relayer": {
    "max_retries": 3,
    "retry_backoff": "1s"
  }
}
```

**High-Volume Deployments:**
```json
{
  "oz_relayer": {
    "max_retries": 5,
    "retry_backoff": "2s"
  }
}
```

### Calculating Maximum Wait Time

**Linear (Symbiotic Relay):**
```
max_wait = retry_backoff × (max_retries × (max_retries + 1) / 2)
```

**Exponential (OZ Relayer):**
```
max_wait ≈ retry_backoff × (2^max_retries - 1) × 1.25  # with jitter
```

| Config | Symbiotic Relay | OZ Relayer |
|--------|-----------------|------------|
| 1s / 3 retries | ~6s | ~9s |
| 1s / 5 retries | ~15s | ~39s |
| 2s / 3 retries | ~12s | ~18s |

## Webhook Configuration

The operator receives blockchain events via webhooks from OZ Monitor using native webhook triggers with HMAC-SHA256 authentication.

### Webhook Architecture

```
OZ Monitor → Webhook (HMAC-SHA256) → Operator /webhook/events
```

OZ Monitor watches provider-specific ingress events and sends them directly to each operator.

Ingress events by provider:
1. `layerzero`: `JobAssigned`.
2. `chainlink_ccv`: `CCIPMessageSent`.

### OZ Monitor Trigger Configuration

Webhook triggers are defined in provider-specific templates and copied to `data/generated-config/oz-monitor/triggers/` at startup.

Current template:
1. `config/templates/oz-monitor/triggers/webhook_layerzero.json` (shared webhook trigger template reused across providers)

```json
{
  "webhook_operator_1": {
    "name": "Webhook (Operator 1)",
    "trigger_type": "webhook",
    "config": {
      "url": {
        "type": "plain",
        "value": "http://operator-1:3000/webhook/events"
      },
      "method": "POST",
      "secret": {
        "type": "plain",
        "value": "test-webhook-secret-32-chars-minimum"
      },
      "headers": {
        "Content-Type": "application/json"
      },
      "payload_mode": "raw"
    }
  }
}
```

Key settings:

| Setting | Description |
|---------|-------------|
| `url.value` | Operator webhook endpoint (use Docker service name in compose) |
| `secret.value` | Must match `WEBHOOK_SECRET` in operator's `.env` (update template value before `make configure` if you changed `.env`) |
| `payload_mode` | Must be `"raw"` to send the full event payload |

### OZ Monitor Job Configuration

Monitor config templates are provider-specific and patched during `make configure`.

Examples:
1. `config/templates/oz-monitor/monitors/layerzero_job_assigned.json`
2. `config/templates/oz-monitor/monitors/ccip_message_sent.json`

```json
{
  "triggers": [
    "webhook_operator_1",
    "webhook_operator_2",
    "webhook_operator_3"
  ]
}
```

### Webhook Authentication

Webhooks are authenticated using HMAC-SHA256:

1. OZ Monitor computes `HMAC-SHA256(secret, request_body + timestamp)`
2. Signature is sent in the `X-Signature` header
3. Timestamp (milliseconds since epoch) is sent in the `X-Timestamp` header
4. Operator verifies the signature and that timestamp is within acceptable window

Requests with invalid/missing signatures or expired timestamps are rejected with HTTP 401.

### Webhook Payload Format

OZ Monitor sends the matched event with this structure:

```json
{
  "EVM": {
    "logs": [
      {
        "address": "0x...",
        "topics": ["0x..."],
        "data": "0x...",
        "blockNumber": 123,
        "transactionHash": "0x...",
        "logIndex": 0
      }
    ],
    "matched_on_args": {
      "events": [
        {
          "signature": "JobAssigned(address,bytes,uint256,address)",
          "hex_signature": "0x...",
          "args": [
            { "name": "dvn", "kind": "address", "indexed": true, "value": "0x..." }
          ]
        }
      ]
    },
    "monitor": {
      "name": "LayerZero JobAssigned"
    },
    "network_slug": "anvil-source",
    "transaction": {
      "hash": "0x...",
      "blockHash": "0x...",
      "blockNumber": 123,
      "transactionIndex": 0,
      "from": "0x...",
      "to": "0x..."
    }
  }
}
```

This example is LayerZero-shaped. For the Chainlink CCV provider, `matched_on_args.events[*].signature` corresponds to `CCIPMessageSent(...)` and fields differ accordingly.

## Contract Addresses

After deployment, addresses are written to `data/deploy-data/`:

- `source_contracts.json` - DVN on source chain
- `dest_contracts.json` - DVN on destination chain
- `ccv_source_contracts.json` - SymbioticCCV source + source onRamp/offRamp
- `ccv_dest_contracts.json` - SymbioticCCV destination + destination onRamp/offRamp
- `relay_infra.json` - Symbiotic relay infrastructure (includes Settlement)
- `addresses.env` - All addresses in shell-sourceable format

For manual testing, source the addresses file:

```bash
source data/deploy-data/addresses.env

# Or use the interactive shell with addresses pre-loaded:
make shell
```

Available variables after sourcing:

| Variable | Description |
|----------|-------------|
| `DVN_SOURCE_ADDRESS` | DVN contract on source chain |
| `DVN_DEST_ADDRESS` | DVN contract on destination chain |
| `CCV_SOURCE_ADDRESS` | SymbioticCCV contract on source chain |
| `CCV_DEST_ADDRESS` | SymbioticCCV contract on destination chain |
| `CCV_SOURCE_ONRAMP_ADDRESS` | Source onRamp-compatible ingress contract |
| `CCV_SOURCE_OFFRAMP_ADDRESS` | Source offRamp-compatible verifier entrypoint |
| `CCV_DEST_ONRAMP_ADDRESS` | Destination onRamp-compatible ingress contract |
| `CCV_DEST_OFFRAMP_ADDRESS` | Destination offRamp-compatible submit target |
| `TEST_OAPP_SOURCE_ADDRESS` | Test OApp on source chain |
| `TEST_OAPP_DEST_ADDRESS` | Test OApp on destination chain |
| `SOURCE_RPC_URL` | Source chain RPC (http://localhost:8545) |
| `DEST_RPC_URL` | Destination chain RPC (http://localhost:8546) |
