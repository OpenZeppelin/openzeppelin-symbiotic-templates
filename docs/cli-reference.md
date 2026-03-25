# CLI Reference

Reference for the repo's message-testing interface.

The supported user-facing commands are:

```bash
make send
make watch
make e2e
```

These are backed by `cargo xtask msg ...`.

## Make Commands

### `make send`

Send one test message.

```bash
make send
make send MSG="test message"
make send ENV=local-ccv MSG="ccv hello"
```

### `make watch`

Watch a previously sent message until it lands on the destination chain.

```bash
make watch
make watch ENV=testnet TIMEOUT=300
make watch GUID=0x...
make watch TX=0x...
```

Supported variables:

- `ENV`
- `TIMEOUT`
- `GUID`
- `TX`

### `make e2e`

Send a message, then watch it to completion.

```bash
make e2e
make e2e MSG="custom message"
make e2e ENV=local-ccv MSG="ccv smoke" TIMEOUT=180
```

Supported variables:

- `ENV`
- `MSG`
- `TIMEOUT`

## Direct xtask Commands

Use xtask directly if you want the explicit Rust entrypoint:

```bash
cargo xtask --env local msg send "hello"
cargo xtask --env local msg watch --timeout 120
cargo xtask --env local msg e2e "hello" --timeout 120
```

### `cargo xtask msg send`

```bash
cargo xtask --env local msg send "hello"
cargo xtask --env local msg send "hello" --gas 250000
cargo xtask --env local msg send "hello" --json
```

### `cargo xtask msg watch`

```bash
cargo xtask --env local msg watch
cargo xtask --env local msg watch --id 0x...
cargo xtask --env local msg watch --tx 0x...
cargo xtask --env local msg watch --timeout 300
```

### `cargo xtask msg e2e`

```bash
cargo xtask --env local msg e2e "hello"
cargo xtask --env local msg e2e "hello" --timeout 300
cargo xtask --env local msg e2e "hello" --json
```

## Message Cache

After `send`, xtask saves message details under:

```text
generated/<env>/msg-cache.json
```

`watch` uses this cache when no explicit `--id` or `--tx` is provided.
