mod contracts;
mod sidecar;

use anyhow::{anyhow, Result};
use contracts::SymbioticLayerZeroDVN;
use ethers::{
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
    types::{Address, Bytes},
};
use serde::Deserialize;
use sidecar::{SidecarClient, KEY_TAG_BLS_BN254};
use sha3::{Digest, Keccak256};
use std::{env, io::Read, str::FromStr, sync::Arc};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

// =============================================================================
// OZ Monitor Input Types
// =============================================================================

/// Root input structure from OZ Monitor
#[derive(Debug, Deserialize)]
pub struct MonitorInput {
    pub monitor_match: MonitorMatch,
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct MonitorMatch {
    #[serde(rename = "EVM")]
    pub evm: Option<EvmMatch>,
}

#[derive(Debug, Deserialize)]
pub struct EvmMatch {
    pub monitor: serde_json::Value,
    pub transaction: Transaction,
    pub receipt: serde_json::Value,
    pub logs: Vec<Log>,
    pub network_slug: String,
    pub matched_on: serde_json::Value,
    pub matched_on_args: MatchedOnArgs,
}

#[derive(Debug, Deserialize)]
pub struct Transaction {
    pub hash: String,
    #[serde(rename = "blockNumber")]
    pub block_number: String,
}

#[derive(Debug, Deserialize)]
pub struct Log {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "logIndex")]
    pub log_index: String,
}

#[derive(Debug, Deserialize)]
pub struct MatchedOnArgs {
    pub events: Option<Vec<MatchedEvent>>,
    pub functions: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub struct MatchedEvent {
    pub signature: String,
    pub args: Vec<EventArg>,
    pub hex_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventArg {
    pub name: String,
    pub value: serde_json::Value,
    pub indexed: bool,
    pub kind: String,
}

// =============================================================================
// Job Processing
// =============================================================================

/// Extracted JobAssigned event data
#[derive(Debug, Clone)]
pub struct JobAssignedData {
    pub job_id: [u8; 32],
    pub dst_eid: u32,
    pub payload_hash: [u8; 32],
    pub sender: Address,
    pub packet_header: Vec<u8>,
    pub confirmations: u64,
}

impl JobAssignedData {
    /// Extract from OZ Monitor matched event args
    fn from_matched_event(event: &MatchedEvent) -> Result<Self> {
        if !event.signature.starts_with("JobAssigned") {
            return Err(anyhow!("Not a JobAssigned event"));
        }

        let mut job_id = [0u8; 32];
        let mut dst_eid = 0u32;
        let mut payload_hash = [0u8; 32];
        let mut sender = Address::zero();
        let mut packet_header = Vec::new();
        let mut confirmations = 0u64;

        for arg in &event.args {
            match arg.name.as_str() {
                "jobId" => {
                    let hex_str = arg.value.as_str().ok_or_else(|| anyhow!("jobId not a string"))?;
                    let bytes = hex::decode(hex_str.trim_start_matches("0x"))?;
                    job_id.copy_from_slice(&bytes);
                }
                "dstEid" => {
                    dst_eid = match &arg.value {
                        serde_json::Value::String(s) => s.parse()?,
                        serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("dstEid not u64"))? as u32,
                        _ => return Err(anyhow!("dstEid invalid type")),
                    };
                }
                "payloadHash" => {
                    let hex_str = arg.value.as_str().ok_or_else(|| anyhow!("payloadHash not a string"))?;
                    let bytes = hex::decode(hex_str.trim_start_matches("0x"))?;
                    payload_hash.copy_from_slice(&bytes);
                }
                "sender" => {
                    let addr_str = arg.value.as_str().ok_or_else(|| anyhow!("sender not a string"))?;
                    sender = Address::from_str(addr_str)?;
                }
                "packetHeader" => {
                    let hex_str = arg.value.as_str().ok_or_else(|| anyhow!("packetHeader not a string"))?;
                    packet_header = hex::decode(hex_str.trim_start_matches("0x"))?;
                }
                "confirmations" => {
                    confirmations = match &arg.value {
                        serde_json::Value::String(s) => s.parse()?,
                        serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("confirmations not u64"))?,
                        _ => return Err(anyhow!("confirmations invalid type")),
                    };
                }
                _ => {}
            }
        }

        Ok(JobAssignedData {
            job_id,
            dst_eid,
            payload_hash,
            sender,
            packet_header,
            confirmations,
        })
    }
}

/// Compute the message hash that validators sign
/// messageHash = keccak256(abi.encode(packetHeader, payloadHash))
fn compute_message_hash(packet_header: &[u8], payload_hash: &[u8; 32]) -> [u8; 32] {
    let encoded = ethers::abi::encode(&[
        ethers::abi::Token::Bytes(packet_header.to_vec()),
        ethers::abi::Token::FixedBytes(payload_hash.to_vec()),
    ]);
    Keccak256::digest(&encoded).into()
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("dvn_worker=info".parse()?))
        .with_writer(std::io::stderr) // OZ Monitor expects output to stderr for logs
        .init();

    info!("=== Symbiotic LayerZero DVN Worker ===");
    info!("Reading event from stdin...");

    // Read JSON from stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    // Parse OZ Monitor input
    let monitor_input: MonitorInput = serde_json::from_str(&input)
        .map_err(|e| anyhow!("Failed to parse OZ Monitor input: {}", e))?;

    // Extract EVM match
    let evm_match = monitor_input
        .monitor_match
        .evm
        .ok_or_else(|| anyhow!("No EVM match in input"))?;

    info!("Processing transaction: {}", evm_match.transaction.hash);

    // Extract JobAssigned event from matched_on_args
    let events = evm_match
        .matched_on_args
        .events
        .ok_or_else(|| anyhow!("No matched events in input"))?;

    let job_assigned_event = events
        .iter()
        .find(|e| e.signature.starts_with("JobAssigned"))
        .ok_or_else(|| anyhow!("No JobAssigned event found"))?;

    let job_data = JobAssignedData::from_matched_event(job_assigned_event)?;

    info!(
        "JobAssigned: job_id={}, dst_eid={}, confirmations={}",
        hex::encode(job_data.job_id),
        job_data.dst_eid,
        job_data.confirmations
    );

    // Get configuration from environment
    let dest_rpc_url = env::var("DEST_RPC_URL")
        .map_err(|_| anyhow!("DEST_RPC_URL not set"))?;
    let dest_dvn_address = env::var("DEST_DVN_ADDRESS")
        .map_err(|_| anyhow!("DEST_DVN_ADDRESS not set"))?;
    let sidecar_url = env::var("SIDECAR_URL")
        .map_err(|_| anyhow!("SIDECAR_URL not set"))?;
    let private_key = env::var("PRIVATE_KEY")
        .map_err(|_| anyhow!("PRIVATE_KEY not set"))?;

    // Initialize destination chain provider and signer
    let dest_provider = Provider::<Http>::try_from(&dest_rpc_url)?;
    let chain_id = dest_provider.get_chainid().await?.as_u64();
    let wallet = private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let dest_signer = Arc::new(SignerMiddleware::new(dest_provider, wallet));

    // Initialize sidecar client
    let sidecar = SidecarClient::new(&sidecar_url);

    // Parse destination DVN address
    let dest_dvn = Address::from_str(&dest_dvn_address)?;

    // Process the job
    process_job(&job_data, &sidecar, dest_signer, dest_dvn).await?;

    info!("Job processed successfully");
    Ok(())
}

/// Process a single JobAssigned event
async fn process_job(
    job: &JobAssignedData,
    sidecar: &SidecarClient,
    dest_signer: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    dest_dvn: Address,
) -> Result<()> {
    // 1. Compute message hash
    let message_hash = compute_message_hash(&job.packet_header, &job.payload_hash);
    info!("Message hash: 0x{}", hex::encode(message_hash));

    // 2. Request BLS signature from Symbiotic sidecar
    let message_to_sign = ethers::abi::encode(&[ethers::abi::Token::FixedBytes(message_hash.to_vec())]);

    info!("Requesting BLS signature from sidecar...");
    let sign_result = sidecar.sign_message_wait(KEY_TAG_BLS_BN254, &message_to_sign).await?;

    info!(
        "Aggregation proof received! request_id={}, epoch={}, proof_size={} bytes",
        sign_result.request_id,
        sign_result.epoch,
        sign_result.proof.len()
    );

    // 3. Submit verification to destination chain
    let dvn_contract = SymbioticLayerZeroDVN::new(dest_dvn, dest_signer);

    let packet_header = Bytes::from(job.packet_header.clone());
    let payload_hash = job.payload_hash;
    let confirmations = job.confirmations;
    let epoch = sign_result.epoch;
    let proof_bytes = Bytes::from(sign_result.proof);

    info!("Submitting verification to destination chain DVN at {}...", dest_dvn);

    let tx = dvn_contract
        .submit_verification(packet_header, payload_hash, confirmations, epoch, proof_bytes)
        .send()
        .await?
        .await?;

    match tx {
        Some(receipt) => {
            info!(
                "Verification submitted! tx_hash={}, gas_used={}",
                receipt.transaction_hash,
                receipt.gas_used.unwrap_or_default()
            );
        }
        None => {
            error!("Transaction sent but no receipt received");
            return Err(anyhow!("No transaction receipt"));
        }
    }

    Ok(())
}
