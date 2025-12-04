mod contracts;
mod sidecar;

use anyhow::Result;
use clap::Parser;
use contracts::{JobAssignedEvent, SymbioticLayerZeroDVN};
use ethers::{
    middleware::SignerMiddleware,
    prelude::*,
    providers::{Http, Provider},
    signers::LocalWallet,
    types::{Address, Bytes, H256},
};
use sidecar::{SidecarClient, KEY_TAG_BLS_BN254};
use sha3::{Digest, Keccak256};
use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug, Clone)]
struct Args {
    /// Source chain RPC URL (where JobAssigned events are emitted)
    #[arg(long, env = "SOURCE_RPC_URL", default_value = "http://localhost:8545")]
    source_rpc_url: String,

    /// Destination chain RPC URL (where verification is submitted)
    #[arg(long, env = "DEST_RPC_URL", default_value = "http://localhost:8546")]
    dest_rpc_url: String,

    /// Source chain DVN address
    #[arg(long, env = "SOURCE_DVN_ADDRESS")]
    source_dvn_address: String,

    /// Destination chain DVN address
    #[arg(long, env = "DEST_DVN_ADDRESS")]
    dest_dvn_address: String,

    /// Symbiotic Relay sidecar URL
    #[arg(long, env = "SIDECAR_URL", default_value = "http://localhost:8081")]
    sidecar_url: String,

    /// Private key for signing transactions
    #[arg(long, env = "PRIVATE_KEY")]
    private_key: String,

    /// Source chain endpoint ID
    #[arg(long, env = "SOURCE_EID", default_value = "31337")]
    source_eid: u32,

    /// Destination chain endpoint ID
    #[arg(long, env = "DEST_EID", default_value = "31338")]
    dest_eid: u32,

    /// Poll interval in seconds
    #[arg(long, env = "POLL_INTERVAL", default_value = "5")]
    poll_interval: u64,
}

/// Worker state for tracking processed jobs
struct WorkerState {
    processed_jobs: HashMap<H256, bool>,
    last_block: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("dvn_worker=info".parse()?))
        .init();

    let args = Args::parse();

    info!(target: "dvn_worker", "=== Symbiotic LayerZero DVN Worker ===");
    info!(target: "dvn_worker", "Source RPC: {}", args.source_rpc_url);
    info!(target: "dvn_worker", "Dest RPC: {}", args.dest_rpc_url);
    info!(target: "dvn_worker", "Source DVN: {}", args.source_dvn_address);
    info!(target: "dvn_worker", "Dest DVN: {}", args.dest_dvn_address);
    info!(target: "dvn_worker", "Sidecar: {}", args.sidecar_url);

    // Initialize providers
    let source_provider = Provider::<Http>::try_from(&args.source_rpc_url)?;
    let dest_provider = Provider::<Http>::try_from(&args.dest_rpc_url)?;

    // Initialize signer for destination chain
    let wallet = args
        .private_key
        .parse::<LocalWallet>()?
        .with_chain_id(dest_provider.get_chainid().await?.as_u64());
    let dest_signer = Arc::new(SignerMiddleware::new(dest_provider.clone(), wallet));

    // Initialize sidecar client
    let sidecar = SidecarClient::new(&args.sidecar_url);

    // Check sidecar health
    if sidecar.is_healthy().await {
        info!(target: "dvn_worker", "Sidecar connection healthy");
    } else {
        warn!(target: "dvn_worker", "Sidecar not reachable, will retry on each job");
    }

    // Parse addresses
    let source_dvn_address = Address::from_str(&args.source_dvn_address)?;
    let dest_dvn_address = Address::from_str(&args.dest_dvn_address)?;

    // Initialize worker state
    let mut state = WorkerState {
        processed_jobs: HashMap::new(),
        last_block: source_provider.get_block_number().await?.as_u64(),
    };

    info!(target: "dvn_worker", "Starting from block {}", state.last_block);

    // Main loop
    let poll_interval = Duration::from_secs(args.poll_interval);

    loop {
        match poll_for_jobs(
            &source_provider,
            source_dvn_address,
            &mut state,
            &sidecar,
            dest_signer.clone(),
            dest_dvn_address,
        )
        .await
        {
            Ok(processed) => {
                if processed > 0 {
                    info!(target: "dvn_worker", "Processed {} job(s)", processed);
                }
            }
            Err(e) => {
                error!(target: "dvn_worker", "Error polling for jobs: {}", e);
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Poll for new JobAssigned events and process them
async fn poll_for_jobs(
    source_provider: &Provider<Http>,
    source_dvn: Address,
    state: &mut WorkerState,
    sidecar: &SidecarClient,
    dest_signer: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    dest_dvn: Address,
) -> Result<usize> {
    let latest_block = source_provider.get_block_number().await?.as_u64();

    if latest_block <= state.last_block {
        return Ok(0);
    }

    // JobAssigned event signature
    let event_sig = H256::from_slice(&Keccak256::digest(
        "JobAssigned(bytes32,uint32,bytes32,address,bytes,uint64)",
    ));

    // Query logs
    let filter = Filter::new()
        .address(source_dvn)
        .topic0(event_sig)
        .from_block(state.last_block + 1)
        .to_block(latest_block);

    let logs = source_provider.get_logs(&filter).await?;

    info!(
        target: "dvn_worker",
        "Scanned blocks {}-{}, found {} JobAssigned event(s)",
        state.last_block + 1,
        latest_block,
        logs.len()
    );

    let mut processed = 0;

    for log in logs {
        let job_id = match log.topics.get(1) {
            Some(id) => *id,
            None => continue,
        };

        // Skip already processed jobs
        if state.processed_jobs.contains_key(&job_id) {
            continue;
        }

        // Parse event
        match JobAssignedEvent::from_log(&log) {
            Some(event) => {
                info!(
                    target: "dvn_worker",
                    "Processing job {} -> dstEid={}, confirmations={}",
                    hex::encode(event.job_id),
                    event.dst_eid,
                    event.confirmations
                );

                match handle_job_assigned(&event, sidecar, dest_signer.clone(), dest_dvn).await {
                    Ok(_) => {
                        state.processed_jobs.insert(job_id, true);
                        processed += 1;
                        info!(target: "dvn_worker", "Job {} completed successfully", hex::encode(event.job_id));
                    }
                    Err(e) => {
                        error!(target: "dvn_worker", "Failed to process job {}: {}", hex::encode(event.job_id), e);
                    }
                }
            }
            None => {
                warn!(target: "dvn_worker", "Failed to parse JobAssigned event from log");
            }
        }
    }

    state.last_block = latest_block;
    Ok(processed)
}

/// Handle a single JobAssigned event
async fn handle_job_assigned(
    event: &JobAssignedEvent,
    sidecar: &SidecarClient,
    dest_signer: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    dest_dvn: Address,
) -> Result<()> {
    // 1. Build the message that validators will sign: keccak256(packetHeader, payloadHash)
    let message_hash = compute_message_hash(&event.packet_header, &event.payload_hash);
    info!(target: "dvn_worker", "Message hash: 0x{}", hex::encode(message_hash));

    // 2. Request signature from Symbiotic sidecar and wait for aggregation proof
    // The message sent to sidecar is abi.encode(messageHash)
    let message_to_sign = ethers::abi::encode(&[ethers::abi::Token::FixedBytes(message_hash.to_vec())]);

    info!(target: "dvn_worker", "Requesting BLS signature from sidecar (streaming wait)...");
    let sign_result = sidecar.sign_message_wait(KEY_TAG_BLS_BN254, &message_to_sign).await?;
    info!(
        target: "dvn_worker",
        "Aggregation proof received! request_id={}, epoch={}, proof_size={} bytes",
        sign_result.request_id,
        sign_result.epoch,
        sign_result.proof.len()
    );

    // 3. Submit verification on destination chain
    let dvn_contract = SymbioticLayerZeroDVN::new(dest_dvn, dest_signer.clone());

    let packet_header = Bytes::from(event.packet_header.clone());
    let payload_hash: [u8; 32] = event.payload_hash;
    let confirmations = event.confirmations;
    let epoch = sign_result.epoch; // u48 fits in u64
    let proof_bytes = Bytes::from(sign_result.proof);

    info!(
        target: "dvn_worker",
        "Submitting verification to destination chain DVN at {}...",
        dest_dvn
    );

    let tx = dvn_contract
        .submit_verification(
            packet_header,
            payload_hash,
            confirmations,
            epoch,
            proof_bytes,
        )
        .send()
        .await?
        .await?;

    match tx {
        Some(receipt) => {
            info!(
                target: "dvn_worker",
                "Verification submitted! tx_hash={}, gas_used={}",
                receipt.transaction_hash,
                receipt.gas_used.unwrap_or_default()
            );
        }
        None => {
            warn!(target: "dvn_worker", "Transaction sent but no receipt received");
        }
    }

    Ok(())
}

/// Compute the message hash that validators sign
/// messageHash = keccak256(abi.encode(packetHeader, payloadHash))
fn compute_message_hash(packet_header: &[u8], payload_hash: &[u8; 32]) -> [u8; 32] {
    // abi.encode(bytes, bytes32) - dynamic bytes followed by fixed bytes32
    let encoded = ethers::abi::encode(&[
        ethers::abi::Token::Bytes(packet_header.to_vec()),
        ethers::abi::Token::FixedBytes(payload_hash.to_vec()),
    ]);

    let hash = Keccak256::digest(&encoded);
    hash.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_message_hash() {
        let packet_header = vec![0x01, 0x02, 0x03];
        let payload_hash = [0xab; 32];

        let hash = compute_message_hash(&packet_header, &payload_hash);
        assert_eq!(hash.len(), 32);

        // Same inputs should produce same hash
        let hash2 = compute_message_hash(&packet_header, &payload_hash);
        assert_eq!(hash, hash2);
    }
}
