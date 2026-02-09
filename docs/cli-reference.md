# CLI Reference

Reference for `scripts/msg`, the devnet message testing tool.

## Commands

### send

Send a test message to the destination chain via `TestOApp`.

```bash
./scripts/msg send                       # Send "hello"
./scripts/msg send -m "test message"     # Send custom message
./scripts/msg send "my message"          # Positional arg = message
./scripts/msg send --dry-run             # Show underlying cast commands
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--message` | `-m` | `"hello"` | Message content |
| `--dry-run` | | | Print cast/curl commands without executing |
| `--json` | | | Machine-readable JSON output |

After sending, the TX hash and block number are cached to `.cache/last-message.json` for use by `status` and `watch`.

### status

One-shot status check across all 3 operators.

```bash
./scripts/msg status                     # Check last sent message (from cache)
./scripts/msg status -g 0xabc...         # Check specific GUID
./scripts/msg status --tx 0xdef...       # Find by source TX hash
./scripts/msg status --json              # JSON output
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--guid` | `-g` | from cache | Message GUID to check |
| `--tx` | `-t` | from cache | Source TX hash to find message |
| `--json` | | | Machine-readable JSON output |
| `--dry-run` | | | Print curl commands without executing |

Positional argument is treated as GUID.

### watch

Poll operators until the message is verified on the destination chain.

```bash
./scripts/msg watch                      # Watch last sent message
./scripts/msg watch -g 0xabc...          # Watch specific GUID
./scripts/msg watch --timeout 300        # Wait up to 5 minutes
./scripts/msg watch -v                   # Verbose output
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--guid` | `-g` | from cache | Message GUID to watch |
| `--tx` | `-t` | from cache | Source TX hash to find message |
| `--timeout` | | `120` | Max wait time in seconds |
| `--verbose` | `-v` | | Show streaming logs |
| `--json` | | | Machine-readable JSON output |
| `--dry-run` | | | Print polling commands without executing |

Positional argument is treated as GUID. Exits 0 on verification, 1 on timeout.

### e2e

Combined send + watch. Sends a message and watches until DVN verification.

```bash
./scripts/msg e2e                        # Full E2E test
./scripts/msg e2e -m "custom msg"        # Custom message
./scripts/msg e2e -v                     # Verbose output
./scripts/msg e2e --timeout 300          # Longer timeout
```

Accepts all flags from both `send` and `watch`.

## Makefile Integration

The Makefile provides shortcuts that wrap `scripts/msg`:

```bash
make send                    # ./scripts/msg send --message "hello"
make send MSG="test"         # ./scripts/msg send --message "test"
make watch                   # ./scripts/msg watch
make watch GUID=0x...        # ./scripts/msg watch --guid 0x...
make watch TIMEOUT=300       # ./scripts/msg watch --timeout 300
make status-msg              # ./scripts/msg status
make status-msg GUID=0x...   # ./scripts/msg status --guid 0x...
make e2e                     # ./scripts/msg e2e
make e2e MSG="test" VERBOSE=1  # ./scripts/msg e2e --message "test" --verbose
```

## Message Cache

After `send`, the tool saves message details to `.cache/last-message.json`:

```json
{
  "tx_hash": "0x...",
  "block": 42,
  "guid": null,
  "message": "hello",
  "dest_eid": 40232
}
```

The `status` and `watch` commands auto-load this file when no `--guid` or `--tx` is specified. The GUID is populated once operators process the message.

## Underlying Commands

Use `--dry-run` on any command to see the underlying calls:

- **send**: `cast send <TestOApp> "send(uint32,string,bytes)" ...` with quoted fee
- **status**: `curl http://localhost:300N/debug/v1/messages` for each operator
- **watch**: Polls operators + `cast logs` on destination chain for DVN events
