use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::{Address, B256};
use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};

use super::{generate_proof_response, verify_merkle_proof, PreparedSubmission, Provider};
use crate::api::AppState;
use crate::config::{AppConfig, LayerZeroConfig};
use crate::crypto::{compute_dvn_leaf, encode_signing_message, MerkleProof};
use crate::error::ProviderError;
use crate::evm::{job_assigned_topic, DecodedJobAssigned};
use crate::storage::MerkleTreeData;
use crate::storage::{MessageData, MessageMetadata, Storage};
use crate::submitter::dvn::{build_signature, encode_submit_proof};
use crate::webhook::{ProofResponse, WebhookEvent};

/// LayerZero provider implementation
pub struct LayerZeroProvider {
    config: LayerZeroConfig,
    app_config: Arc<AppConfig>,
    storage: Arc<Storage>,
}

impl LayerZeroProvider {
    /// Create a new LayerZero provider
    pub fn new(config: LayerZeroConfig, app_config: Arc<AppConfig>, storage: Arc<Storage>) -> Self {
        Self {
            config,
            app_config,
            storage,
        }
    }

    /// Validate event against configuration
    /// OZ Monitor handles source chain filtering, we only check destination
    fn valid_event(&self, _src_chain: u64, dst_eid: u32) -> bool {
        // Check if destination EID maps to a configured destination chain
        let dst_chain_id = match self.config.eid_to_chain_id.get(&dst_eid) {
            Some(id) => *id,
            None => return false,
        };

        self.app_config.is_supported_destination(dst_chain_id)
    }
}

#[async_trait]
impl Provider for LayerZeroProvider {
    fn name(&self) -> &'static str {
        "layerzero"
    }

    async fn handle_webhook_event(&self, event: &WebhookEvent) -> Result<(), ProviderError> {
        let job_assigned_topic = job_assigned_topic();

        for log in &event.evm.logs {
            // Check if this is a JobAssigned event
            if log.topics.is_empty() || log.topics[0] != job_assigned_topic {
                continue;
            }

            // Decode the event (DVN 11-field format)
            let alloy_log = log.to_alloy_log();
            let job_event = DecodedJobAssigned::decode_log(&alloy_log).map_err(|e| {
                ProviderError::EventDecode(format!("failed to decode JobAssigned: {}", e))
            })?;

            // Get source chain ID from src_eid (DVN includes this directly)
            // Fall back to transaction chain_id if EID mapping not found
            let src_chain_id = self
                .config
                .eid_to_chain_id
                .get(&job_event.src_eid)
                .copied()
                .or_else(|| {
                    event
                        .evm
                        .transaction
                        .as_ref()
                        .and_then(|tx| tx.chain_id)
                })
                .ok_or(ProviderError::MissingTransaction)?;

            // Validate event against configuration (matches Go's validEvent)
            if !self.valid_event(src_chain_id, job_event.dst_eid) {
                tracing::debug!(
                    tx_hash = ?log.transaction_hash,
                    block = log.block_number,
                    guid = %job_event.guid,
                    src_eid = job_event.src_eid,
                    dst_eid = job_event.dst_eid,
                    src_chain = src_chain_id,
                    "ignoring invalid JobAssigned event"
                );
                continue;
            }

            // Map destination EID to chain ID - log and continue instead of aborting
            let dst_chain_id = match self.config.eid_to_chain_id.get(&job_event.dst_eid) {
                Some(id) => id,
                None => {
                    tracing::warn!(
                        dst_eid = job_event.dst_eid,
                        guid = %job_event.guid,
                        "unknown destination EID, skipping event"
                    );
                    continue;
                }
            };

            // Create message using guid as unique identifier (DVN)
            let message = MessageData {
                metadata: MessageMetadata {
                    source_chain: src_chain_id,
                    destination_chain: *dst_chain_id,
                    block_number: log.block_number,
                    message_id: job_event.message_id(), // guid is the unique identifier in DVN
                    event_tx_hash: log.transaction_hash,
                    ttl: None,
                },
                data: serde_json::to_vec(&job_event).unwrap_or_default(),
            };

            // Save (idempotent - duplicates ignored)
            self.storage.save_message(&message)?;

            tracing::info!(
                guid = %job_event.guid,
                src_eid = job_event.src_eid,
                dst_eid = job_event.dst_eid,
                src = src_chain_id,
                dst = dst_chain_id,
                nonce = job_event.nonce,
                block = log.block_number,
                "stored JobAssigned event (DVN)"
            );
        }

        Ok(())
    }

    fn register_api_routes(&self, router: Router<AppState>) -> Router<AppState> {
        router
            .route("/api/v1/layerzero/proof", post(get_proof_handler))
            .route("/api/v1/layerzero/verify", post(verify_proof_handler))
    }

    async fn acceptance_hook(&self, _msg: &MessageData) -> Result<(), ProviderError> {
        // LayerZero messages are accepted by default
        // Custom validation can be added here
        Ok(())
    }

    fn compute_leaf_hash(&self, message: &MessageData) -> Result<B256, ProviderError> {
        let job_assigned: DecodedJobAssigned = serde_json::from_slice(&message.data)?;
        Ok(compute_dvn_leaf(
            &job_assigned.packet_header,
            job_assigned.payload_hash,
            job_assigned.confirmations,
        ))
    }

    fn encode_signing_message(&self, tree: &MerkleTreeData) -> Result<Vec<u8>, ProviderError> {
        let target_address_str = self
            .config
            .dvn_addresses
            .get(&tree.destination_chain)
            .ok_or_else(|| {
                ProviderError::EventDecode(format!(
                    "DVN address not configured for chain {}",
                    tree.destination_chain
                ))
            })?;

        let target_address: Address = target_address_str.parse().map_err(|e| {
            ProviderError::EventDecode(format!("invalid DVN address: {e}"))
        })?;

        Ok(encode_signing_message(
            tree.destination_chain,
            target_address,
            tree.root_hash,
        ))
    }

    fn prepare_submission(
        &self,
        message: &MessageData,
        tree: &MerkleTreeData,
        proof: &MerkleProof,
        target_address: &str,
    ) -> Result<PreparedSubmission, ProviderError> {
        let job_assigned: DecodedJobAssigned = serde_json::from_slice(&message.data)?;
        let epoch = tree
            .epoch
            .ok_or_else(|| ProviderError::EventDecode("missing epoch on signed tree".to_string()))?;

        let signature = build_signature(epoch, &tree.proof);
        let calldata = encode_submit_proof(
            &job_assigned.packet_header,
            job_assigned.payload_hash,
            job_assigned.confirmations,
            proof.siblings.clone(),
            tree.root_hash,
            signature,
        );

        let to = if target_address.is_empty() {
            self.config
                .dvn_addresses
                .get(&tree.destination_chain)
                .cloned()
                .ok_or_else(|| {
                    ProviderError::EventDecode(format!(
                        "missing destination target for chain {}",
                        tree.destination_chain
                    ))
                })?
        } else {
            target_address.to_string()
        };

        Ok(PreparedSubmission {
            to,
            calldata: calldata.to_vec(),
        })
    }
}

// API handler types for LayerZero endpoints

/// Request body for proof endpoint
#[derive(Debug, serde::Deserialize)]
pub struct ProofRequest {
    pub message_ids: Vec<B256>,
}

/// Handler for getting proofs
async fn get_proof_handler(
    State(state): State<AppState>,
    Json(req): Json<ProofRequest>,
) -> Result<Json<HashMap<B256, ProofResponse>>, axum::http::StatusCode> {
    let mut results = HashMap::new();

    for id in req.message_ids {
        match generate_proof_response(&state.storage, &id) {
            Ok(Some(proof)) => {
                results.insert(id, proof);
            }
            Ok(None) => {
                // Message not found - skip
            }
            Err(e) => {
                tracing::error!(error = %e, message_id = %id, "error generating proof");
            }
        }
    }

    Ok(Json(results))
}

/// Handler for verifying proofs
async fn verify_proof_handler(Json(proof): Json<ProofResponse>) -> Json<String> {
    let is_valid = verify_merkle_proof(&proof);
    if is_valid {
        Json("valid".to_string())
    } else {
        Json("invalid".to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn test_storage() -> (Arc<Storage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        (Arc::new(storage), dir)
    }

    fn test_lz_config() -> LayerZeroConfig {
        LayerZeroConfig {
            eid_to_chain_id: {
                let mut map = HashMap::new();
                map.insert(30101, 1); // Ethereum mainnet
                map.insert(30110, 42161); // Arbitrum
                map.insert(40231, 31337); // Local src
                map.insert(40232, 31338); // Local dst
                map
            },
            dvn_addresses: {
                let mut map = HashMap::new();
                map.insert(31338, "0x1234567890123456789012345678901234567890".to_string());
                map.insert(42161, "0xabcdef0123456789abcdef0123456789abcdef01".to_string());
                map
            },
        }
    }

    fn test_app_config() -> Arc<AppConfig> {
        use crate::config::*;
        use std::time::Duration;

        Arc::new(AppConfig {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 3000,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                idle_timeout: Duration::from_secs(120),
                security: SecurityConfig::default(),
            },
            database: DatabaseConfig {
                path: "./data/test.db".to_string(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "json".to_string(),
            },
            symbiotic_relay: SymbioticRelayConfig {
                address: "http://localhost:50051".to_string(),
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
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338, 42161],
            provider: "layerzero".to_string(),
            layerzero: Some(test_lz_config()),
            chainlink_ccv: None,
        })
    }

    #[test]
    fn test_layerzero_provider_new() {
        let (storage, _dir) = test_storage();
        let config = test_app_config();
        let lz_config = test_lz_config();

        let provider = LayerZeroProvider::new(lz_config, config, storage);
        assert_eq!(provider.name(), "layerzero");
    }

    #[test]
    fn test_valid_event_supported_destination() {
        let (storage, _dir) = test_storage();
        let config = test_app_config();
        let lz_config = test_lz_config();

        let provider = LayerZeroProvider::new(lz_config, config, storage);

        // 40232 maps to 31338 which is in destination_chains
        assert!(provider.valid_event(1, 40232));
    }

    #[test]
    fn test_valid_event_unsupported_destination() {
        let (storage, _dir) = test_storage();
        let config = test_app_config();
        let lz_config = test_lz_config();

        let provider = LayerZeroProvider::new(lz_config, config, storage);

        // 30101 maps to chain 1 which is NOT in destination_chains
        assert!(!provider.valid_event(31337, 30101));
    }

    #[test]
    fn test_valid_event_eid_not_found() {
        let (storage, _dir) = test_storage();
        let config = test_app_config();
        let lz_config = test_lz_config();

        let provider = LayerZeroProvider::new(lz_config, config, storage);

        // Unknown EID
        assert!(!provider.valid_event(1, 99999));
    }

    #[tokio::test]
    async fn test_acceptance_hook_passthrough() {
        let (storage, _dir) = test_storage();
        let config = test_app_config();
        let lz_config = test_lz_config();

        let provider = LayerZeroProvider::new(lz_config, config, storage);

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 12345,
                message_id: B256::from_slice(&[0x01u8; 32]),
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: vec![],
        };

        // LayerZero provider accepts all messages by default
        let result = provider.acceptance_hook(&msg).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_proof_request_deserialization() {
        let json = r#"{"message_ids": ["0x0101010101010101010101010101010101010101010101010101010101010101"]}"#;
        let req: ProofRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message_ids.len(), 1);
        assert_eq!(req.message_ids[0], B256::from_slice(&[0x01u8; 32]));
    }

    #[test]
    fn test_proof_request_empty() {
        let json = r#"{"message_ids": []}"#;
        let req: ProofRequest = serde_json::from_str(json).unwrap();
        assert!(req.message_ids.is_empty());
    }

    #[test]
    fn test_proof_request_multiple_ids() {
        let json = r#"{"message_ids": [
            "0x0101010101010101010101010101010101010101010101010101010101010101",
            "0x0202020202020202020202020202020202020202020202020202020202020202"
        ]}"#;
        let req: ProofRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message_ids.len(), 2);
    }
}
