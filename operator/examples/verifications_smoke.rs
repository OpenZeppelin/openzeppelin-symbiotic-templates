//! Tier-1 smoke server for `GET /verifications`.
//!
//! Boots the operator's axum router with an in-process seeded storage so you
//! can curl the endpoint without docker, the Symbiotic sidecar, or the OZ
//! Relayer. Validates that the route is mounted under the real router,
//! serialization matches the canonical wire format, and the 200/400/404
//! response paths behave correctly.
//!
//! ```
//! cargo run -p operator --example verifications_smoke
//! ```
//!
//! Curl examples are printed on startup.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, B256, keccak256};
use operator::api::{AppState, create_router};
use operator::config::{
    AppConfig, DatabaseConfig, LoggingConfig, OzRelayerConfig, SecurityConfig, ServerConfig,
    SignerConfig, SymbioticRelayConfig,
};
use operator::evm::DecodedCcipMessageSent;
use operator::provider::ChainlinkCcvProvider;
use operator::provider::chainlink_ccv::compute_ccv_and_executor_hash;
use operator::storage::{MerkleTreeData, MessageData, MessageMetadata, Storage};

const SEEDED_ID_HEX: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt::init();

    // Local-only ephemeral DB. Removed when the process exits.
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("redb");
    let storage = Arc::new(Storage::new_with_provider(&db_path, "chainlink_ccv")?);

    // Address layout mirrors the canonical CCIP receipt structure for a
    // single-CCV, no-token-transfer message: [SourceCCV, Executor, NetworkFee].
    let source_ccv = Address::new([0x44u8; 20]);
    let destination_ccv = Address::new([0x22u8; 20]);
    let source_onramp = Address::new([0x11u8; 20]);
    let destination_offramp = Address::new([0x33u8; 20]);
    let executor = Address::new([0x77u8; 20]);
    let network_fee = Address::new([0xFFu8; 20]);

    let ccv_config = operator::provider::types::ChainlinkCcvConfig {
        source_chain_id: 31337,
        destination_chain_id: 31338,
        source_chain_selector: 11_111,
        destination_chain_selector: 22_222,
        source_ccv_address: format!("{:#x}", source_ccv),
        destination_ccv_address: format!("{:#x}", destination_ccv),
        source_onramp_address: format!("{:#x}", source_onramp),
        destination_offramp_address: format!("{:#x}", destination_offramp),
    };

    let app_config = Arc::new(AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(120),
            security: SecurityConfig::default(),
        },
        database: DatabaseConfig {
            path: db_path.to_string_lossy().into_owned(),
        },
        logging: LoggingConfig {
            level: "info".into(),
            format: "pretty".into(),
        },
        symbiotic_relay: SymbioticRelayConfig {
            address: "http://localhost:50051".into(),
            key_tag: 15,
            use_mock: true,
            max_retries: 3,
            timeout: Duration::from_secs(30),
            retry_backoff: Duration::from_secs(1),
        },
        signer: SignerConfig {
            event_poll_interval: Duration::from_secs(15),
            sign_job_interval: Duration::from_secs(1),
            sign_worker_count: 2,
            min_batch_size: 1,
            acceptance_hooks: Vec::new(),
        },
        oz_relayer: OzRelayerConfig::default(),
        destination_chains: vec![31338],
        provider: "chainlink_ccv".into(),
        layerzero: None,
        chainlink_ccv: Some(ccv_config.clone()),
        finality_gating: false,
        source_rpc_url: None,
        sweep: operator::config::SweepSettings::default(),
    });

    let provider: operator::provider::DynProvider = Arc::new(ChainlinkCcvProvider::new(
        ccv_config,
        Arc::clone(&app_config),
        Arc::clone(&storage),
    )?);

    let seeded_id: B256 = SEEDED_ID_HEX.parse()?;
    seed_attested_message(&storage, seeded_id, source_ccv, executor, network_fee)?;

    let state = AppState {
        storage: Arc::clone(&storage),
        provider: Arc::clone(&provider),
        config: Arc::clone(&app_config),
        start_time: Instant::now(),
    };
    let router = create_router(state);

    let addr: std::net::SocketAddr = "127.0.0.1:3000".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    print_curl_recipes(seeded_id);
    println!("smoke server listening on http://{addr} — Ctrl-C to stop");

    axum::serve(listener, router).await?;
    Ok(())
}

/// Seed storage with a signed+attested merkle tree for `message_id`, with
/// `receipt_issuers` populated per the canonical OnRamp layout
/// `[SourceCCV, Executor, NetworkFee]`. Mirrors the production data path so
/// `build_verifier_result` resolves cleanly.
fn seed_attested_message(
    storage: &Storage,
    message_id: B256,
    source_ccv: Address,
    executor: Address,
    network_fee: Address,
) -> eyre::Result<()> {
    let version = [0x1au8, 0x75, 0xbd, 0x93];
    // The OnRamp would emit a message with ccvAndExecutorHash already computed
    // from these receipts; mirror that exactly so the served payload would
    // pass the indexer's ValidateCCVAndExecutorHash.
    let hash = compute_ccv_and_executor_hash(&[source_ccv], executor);
    let msg_event = DecodedCcipMessageSent {
        dest_chain_selector: 22_222,
        sender: Address::ZERO,
        message_id,
        fee_token: Address::ZERO,
        encoded_message: minimal_message_v1_bytes(hash),
        verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        receipt_issuers: vec![source_ccv, executor, network_fee],
    };
    let message = MessageData {
        metadata: MessageMetadata {
            source_chain: 31337,
            destination_chain: 31338,
            block_number: 100,
            message_id,
            event_tx_hash: B256::ZERO,
            ttl: None,
        },
        data: serde_json::to_vec(&msg_event)?,
    };
    storage.save_message(&message)?;

    // Signing message = keccak256(version || message_id). Matches
    // `ChainlinkCcvProvider::compute_leaf_hash`.
    let mut signing = Vec::with_capacity(36);
    signing.extend_from_slice(&version);
    signing.extend_from_slice(message_id.as_slice());
    let root = keccak256(&signing);

    let tree = MerkleTreeData {
        root_hash: root,
        message_ids: vec![message_id],
        leaf_hashes: vec![root],
        source_chain: 31337,
        destination_chain: 31338,
        block_numbers: vec![100],
        proof: vec![0xBEu8; 96],
        epoch: Some(42),
        attested_at: Some(1_700_000_000),
    };
    storage.save_merkle_tree(&tree)?;
    Ok(())
}

/// Build a minimal valid CCIP v1.7 packed MessageV1 with no dynamic fields.
fn minimal_message_v1_bytes(ccv_and_executor_hash: B256) -> Vec<u8> {
    let mut buf = Vec::with_capacity(79);
    buf.push(1u8); // version
    buf.extend_from_slice(&11_111u64.to_be_bytes()); // source_chain_selector
    buf.extend_from_slice(&22_222u64.to_be_bytes()); // dest_chain_selector
    buf.extend_from_slice(&7u64.to_be_bytes()); // sequence_number
    buf.extend_from_slice(&50_000u32.to_be_bytes()); // execution_gas_limit
    buf.extend_from_slice(&200_000u32.to_be_bytes()); // ccip_receive_gas_limit
    buf.extend_from_slice(&0u32.to_be_bytes()); // finality
    buf.extend_from_slice(ccv_and_executor_hash.as_slice()); // 32 bytes
    buf.push(0); // on_ramp_address_length
    buf.push(0); // off_ramp_address_length
    buf.push(0); // sender_length
    buf.push(0); // receiver_length
    buf.extend_from_slice(&0u16.to_be_bytes()); // dest_blob_length
    buf.extend_from_slice(&0u16.to_be_bytes()); // token_transfer_length
    buf.extend_from_slice(&0u16.to_be_bytes()); // data_length
    buf
}

fn print_curl_recipes(seeded: B256) {
    let zero = format!("0x{}", "00".repeat(32));
    println!();
    println!("Try these:");
    println!();
    println!("  # Missing messageID → 400");
    println!("  curl -i 'http://localhost:3000/verifications'");
    println!();
    println!("  # Malformed id → 400");
    println!("  curl -i 'http://localhost:3000/verifications?messageID=not-a-b256'");
    println!();
    println!("  # Unknown id → 404 with canonical envelope");
    println!("  curl -is 'http://localhost:3000/verifications?messageID={zero}' | tail -n +1");
    println!();
    println!("  # Seeded id → 200 with full VerifierResult");
    println!("  curl -s 'http://localhost:3000/verifications?messageID={seeded:#x}' | jq .");
    println!();
    println!("  # Mixed batch (known + unknown) → 200 with both results[] and errors[]");
    println!(
        "  curl -s 'http://localhost:3000/verifications?messageID={seeded:#x}&messageID={zero}' | jq ."
    );
    println!();
    println!("  # Oversized batch (21 ids) → 400");
    println!("  URL='http://localhost:3000/verifications?'");
    println!("  for i in $(seq 1 21); do URL=\"${{URL}}messageID=0x$(printf '%064d' $i)&\"; done");
    println!("  curl -i \"${{URL%&}}\"");
    println!();
}
