# Operator Guide

Development guide for the Rust operator service.

## Module Overview

| Module | File | Purpose |
|--------|------|---------|
| **API Server** | `src/api/` | Axum HTTP server, webhook endpoint, debug routes |
| **Provider** | `src/provider/mod.rs` | `Provider` trait, provider registration |
| **LayerZeroProvider** | `src/provider/layerzero.rs` | Decodes `JobAssigned` events, stores messages |
| **SignerJob** | `src/signer/mod.rs` | Batches messages into merkle trees, requests BLS signatures |
| **RelaySubmitterJob** | `src/relay_submitter/mod.rs` | Submits signed proofs via OZ Relayer |
| **Storage** | `src/storage/mod.rs` | redb key-value store (messages, merkle trees, submissions) |
| **Crypto** | `src/crypto/mod.rs` | Merkle tree construction, DVN leaf hashing, signing message encoding |
| **Symbiotic Relay** | `src/symbiotic_relay/` | gRPC client for BLS signing sidecar |
| **Relayer Client** | `src/relayer_client/` | HTTP client for OZ Relayer transaction submission |
| **Config** | `src/config/mod.rs` | YAML/JSON configuration parsing |

## Data Flow

1. **Webhook ingestion** - OZ Monitor sends an HMAC-authenticated webhook to the API server. The provider decodes the event and stores the message as `Pending` in redb.

2. **Merkle tree creation** - SignerJob polls for `Pending` messages, groups them by (source chain, destination chain), runs them through the provider's `acceptance_hook`, builds a merkle tree of DVN-compatible leaf hashes, and marks messages `Processing`.

3. **BLS signing** - SignerJob sends the merkle root (ABI-encoded with chain ID and DVN address) to the Symbiotic Relay sidecar for BLS signing. On aggregation success, the proof is attached to the tree and messages are marked `Signed`.

4. **Proof submission** - RelaySubmitterJob finds signed trees, generates per-message merkle proofs, encodes `submitProof` calldata, and submits via OZ Relayer. Messages are marked `Submitted`.

5. **Confirmation** - RelaySubmitterJob polls OZ Relayer for transaction status. On-chain confirmation marks messages `Confirmed`.

## Background Jobs

### SignerJob

Runs three concurrent tasks:

- **Message processing loop** (`event_poll_interval`, default 15s) - Polls storage for `Pending` messages, builds merkle trees, enqueues roots for signing.
- **Worker pool** (`sign_worker_count`, default 2) - Workers consume merkle root work items from an mpsc channel. Each worker submits a signing request to Symbiotic Relay, then polls for the aggregation proof.
- **Periodic sync loop** (`sign_job_interval`, default 1s) - Re-enqueues pending merkle roots that haven't been signed yet (retries after transient failures).

### RelaySubmitterJob

Runs two concurrent tasks:

- **Submission loop** (`oz_relayer.poll_interval`) - Finds signed trees without submissions, encodes calldata, submits to OZ Relayer with idempotency keys.
- **Status poll loop** (`oz_relayer.status_poll_interval`) - Polls OZ Relayer for status updates on pending submissions. Updates storage when transactions are confirmed or failed.

## Adding a New Provider

1. Create `operator/src/provider/yourprovider.rs` implementing the `Provider` trait:

```rust
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn handle_webhook_event(&self, event: &WebhookEvent) -> Result<(), ProviderError>;

    // Optional overrides:
    fn register_api_routes(&self, router: Router<AppState>) -> Router<AppState> { router }
    async fn acceptance_hook(&self, _msg: &MessageData) -> Result<(), ProviderError> { Ok(()) }
}
```

2. Add configuration to `src/config/mod.rs`.

3. Register in `create_provider()` in `src/provider/mod.rs`:

```rust
match config.provider.to_lowercase().as_str() {
    "layerzero" => { /* existing */ }
    "yourprovider" => Ok(Arc::new(YourProvider::new(config, storage))),
    // ...
}
```

4. The provider must store messages using `storage.save_message()` with a unique `message_id`. The signer and submitter jobs are protocol-agnostic and work automatically from storage.

## Running Locally

Use `make dev-operator` to run operator-1 outside Docker for fast iteration:

```bash
# Start the full stack first
make start

# Run operator-1 locally (replaces the Docker container)
make dev-operator
```

This runs `cargo run` with the generated config at `data/generated-config/operator-1/config.json` and `RUST_LOG=debug`.

The local operator connects to the same Docker services (anvil, symbiotic-relay, oz-relayer) and receives the same webhooks from oz-monitor.

## Key Config Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `signer.event_poll_interval` | 15s | How often to check for new pending messages |
| `signer.sign_job_interval` | 1s | How often to retry pending merkle roots |
| `signer.sign_worker_count` | 2 | Concurrent signing workers |
| `signer.min_batch_size` | 1 | Minimum messages before creating a tree |
| `oz_relayer.poll_interval` | 5s | How often to check for signed trees to submit |
| `oz_relayer.status_poll_interval` | 10s | How often to poll OZ Relayer for tx status |
| `symbiotic_relay.key_tag` | 15 | BLS key identifier in the sidecar |
