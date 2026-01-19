# Troubleshooting

Common issues and solutions for the Symbiotic LayerZero DVN template.

## Checking Message Status

Use the debug API to check message processing status:

```bash
# List all messages
curl http://localhost:3001/debug/v1/messages

# Filter by status
curl "http://localhost:3001/debug/v1/messages?status=pending"

# Get specific message
curl http://localhost:3001/debug/v1/messages/0xabc123...
```

## Webhook Issues

### Webhook Not Received

1. Check operator logs for incoming requests:
   ```bash
   make logs-operators | grep webhook
   ```

2. Verify network connectivity (operators must be reachable from OZ Monitor):
   ```bash
   docker compose exec oz-monitor curl -s http://operator-1:3000/healthz
   ```

3. Confirm trigger is linked in monitor config at `config/oz-monitor/monitors/layerzero_job_assigned.json`

### Authentication Failures (401)

1. Verify secrets match between OZ Monitor config and operator `.env`
2. Ensure secret is at least 32 characters
3. Check for trailing whitespace in config files

### Payload Parsing Errors

1. Ensure `payload_mode: "raw"` is set in trigger config
2. Check operator logs for deserialization errors
3. Verify OZ Monitor version supports the expected payload format

## Retry Issues

### "OZ Relayer request failed after all retries exhausted"

The relayer was unavailable or returning persistent errors.

1. Check OZ Relayer service status:
   ```bash
   curl -H "Authorization: Bearer $OZ_RELAYER_API_KEY" http://localhost:8080/api/v1/health
   ```

2. Verify network connectivity from operator container

3. Check relayer logs for errors:
   ```bash
   make logs-relayer
   ```

4. For 429 errors: increase `retry_backoff` or reduce submission rate

### Excessive Latency

If transaction submission is slow:

1. Check current retry configuration
2. Monitor retry frequency in logs:
   ```bash
   make logs-operators | grep -i "retrying"
   ```
3. Consider reducing `max_retries` to fail faster
4. Verify relayer is not rate-limiting your requests

### Intermittent Failures

If some requests fail while others succeed:

1. Network instability: increase `max_retries`
2. Rate limiting: increase `retry_backoff`
3. Check for patterns (time of day, message volume)

## BLS Signing Issues

### Signatures Not Aggregating

1. Check symbiotic-relay sidecar status:
   ```bash
   docker compose ps symbiotic-relay-1
   ```

2. Verify BLS keys are configured correctly in `.env`

3. Check sidecar logs:
   ```bash
   docker compose logs symbiotic-relay-1
   ```

### Quorum Not Reached

1. Verify all required operators are running
2. Check that operators are receiving the same events
3. Verify operator keys are registered in the Settlement contract

## Service Health

### Checking Container Status

```bash
# Overview of all services
make status

# Detailed container info
docker compose ps

# Check specific service
docker compose logs -f operator-1
```

### Service Won't Start

1. Check for missing environment variables:
   ```bash
   docker compose config
   ```

2. Verify secrets are at least 32 characters

3. Check port conflicts:
   ```bash
   lsof -i :3001  # operator port
   lsof -i :8080  # relayer port
   ```

## Network Issues

### Anvil Not Responding

1. Restart anvil services:
   ```bash
   docker compose restart anvil anvil-settlement
   ```

2. Check anvil logs for errors:
   ```bash
   docker compose logs anvil
   ```

### Contracts Not Found

1. Verify deployment completed:
   ```bash
   ls data/deploy-data/
   ```

2. Re-deploy if needed:
   ```bash
   make clean && make start
   ```

## Log Analysis

### Useful Log Commands

```bash
# Follow all operator logs
make logs-operators

# Filter for errors
make logs-operators 2>&1 | grep -i error

# Filter for retry activity
make logs-operators | grep -i "retrying"

# Check for exhausted retries
make logs-operators | grep "retries exhausted"

# Monitor events
make logs-monitor

# Check relayer activity
make logs-relayer
```

### Common Log Patterns

| Pattern | Meaning |
|---------|---------|
| `webhook received` | Event received from OZ Monitor |
| `message batched` | Message added to Merkle tree |
| `signatures aggregated` | BLS quorum reached |
| `proof submitted` | Sent to OZ Relayer |
| `tx confirmed` | On-chain confirmation |

## Migration from Python Script

If upgrading from an older version that used the Python webhook script:

1. Remove `config/oz-monitor/scripts/send_webhook.py` (already deleted)
2. Update trigger config to use `trigger_type: "webhook"` instead of `trigger_type: "script"`
3. Set `payload_mode: "raw"` in the webhook config
4. Ensure `WEBHOOK_SECRET` is set in operator `.env`
5. Restart all services: `make stop && make start`
