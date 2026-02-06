use std::sync::Arc;

use alloy::primitives::B256;
use async_trait::async_trait;
use axum::Router;

use crate::api::AppState;
use crate::config::AppConfig;
use crate::crypto::MerkleProof;
use crate::crypto::generate_proof;
use crate::error::ProviderError;
use crate::storage::{MerkleTreeData, MessageData, Storage};
use crate::webhook::{ProofResponse, WebhookEvent};

pub mod chainlink_ccv;
pub mod layerzero;
pub mod types;

pub use chainlink_ccv::ChainlinkCcvProvider;
pub use layerzero::LayerZeroProvider;

/// Type alias for a thread-safe, dynamically-dispatched provider
pub type DynProvider = Arc<dyn Provider>;

/// Provider-specific submission payload prepared for OZ Relayer.
#[derive(Debug, Clone)]
pub struct PreparedSubmission {
    pub to: String,
    pub calldata: Vec<u8>,
}

/// Provider trait defining the interface for bridge protocol providers.
///
/// This trait mirrors Go's `IProvider` interface for consistency across implementations.
/// Providers handle protocol-specific webhook events, API routes, and message validation.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Provider name for logging and debugging
    fn name(&self) -> &'static str;

    /// Handle incoming webhook events from OZ Monitor
    async fn handle_webhook_event(&self, event: &WebhookEvent) -> Result<(), ProviderError>;

    /// Register provider-specific API routes (optional - default no-op)
    fn register_api_routes(&self, router: Router<AppState>) -> Router<AppState> {
        router
    }

    /// Validate message before signing (optional - default pass-through)
    /// Can be used for whitelisting, amount limits, or protocol-specific validation.
    async fn acceptance_hook(&self, _msg: &MessageData) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Maximum number of messages grouped into a single merkle tree batch.
    fn max_batch_size(&self) -> usize {
        usize::MAX
    }

    /// Compute provider-specific leaf hash used in merkle trees.
    fn compute_leaf_hash(&self, _message: &MessageData) -> Result<B256, ProviderError> {
        Err(ProviderError::UnknownEvent(
            "compute_leaf_hash not implemented".to_string(),
        ))
    }

    /// Encode the bytes to be signed by Symbiotic relay for the tree root.
    fn encode_signing_message(&self, _tree: &MerkleTreeData) -> Result<Vec<u8>, ProviderError> {
        Err(ProviderError::UnknownEvent(
            "encode_signing_message not implemented".to_string(),
        ))
    }

    /// Build provider-specific on-chain submission payload.
    fn prepare_submission(
        &self,
        _message: &MessageData,
        _tree: &MerkleTreeData,
        _proof: &MerkleProof,
        _target_address: &str,
    ) -> Result<PreparedSubmission, ProviderError> {
        Err(ProviderError::UnknownEvent(
            "prepare_submission not implemented".to_string(),
        ))
    }
}

/// Create a provider from configuration
///
/// This is the single registration point for all providers.
/// To add a new provider:
/// 1. Create `provider/yourprovider.rs` implementing the `Provider` trait
/// 2. Add config struct to `config/mod.rs`
/// 3. Add a match arm here
pub fn create_provider(
    config: Arc<AppConfig>,
    storage: Arc<Storage>,
) -> Result<DynProvider, ProviderError> {
    match config.provider.to_lowercase().as_str() {
        "layerzero" => {
            let lz_config = config.layerzero.clone().unwrap_or_default();
            Ok(Arc::new(LayerZeroProvider::new(lz_config, config, storage)))
        }
        "chainlink_ccv" => {
            let ccv_config = config.chainlink_ccv.clone().unwrap_or_default();
            Ok(Arc::new(ChainlinkCcvProvider::new(ccv_config, config, storage)))
        }
        other => Err(ProviderError::UnknownEvent(format!(
            "unknown provider: {}",
            other
        ))),
    }
}

/// Common proof generation logic shared by all providers
pub fn generate_proof_response(
    storage: &Storage,
    message_id: &B256,
) -> Result<Option<ProofResponse>, ProviderError> {
    // Validate message exists
    if storage.get_message(message_id)?.is_none() {
        return Ok(None);
    }

    // Look up root hash directly by message_id
    let root_hash = match storage.get_merkle_root_by_message(message_id)? {
        Some(r) => r,
        None => return Ok(None), // Not yet in a merkle tree
    };

    // Get the merkle tree by root
    let tree = match storage.get_merkle_tree_by_root(&root_hash)? {
        Some(t) => t,
        None => return Ok(None),
    };

    // Find message_id position in parallel arrays
    let position = match tree.message_ids.iter().position(|id| id == message_id) {
        Some(p) => p,
        None => return Ok(None),
    };

    // Get corresponding leaf_hash from parallel arrays (already computed and stored)
    let leaf_hash = tree.leaf_hashes.get(position).copied().ok_or_else(|| {
        ProviderError::EventDecode(format!(
            "leaf_hashes missing for message {} at position {}",
            message_id, position
        ))
    })?;

    // Generate proof with leaf_hashes (sorted, as required by generate_proof)
    let proof = generate_proof(&tree.leaf_hashes, leaf_hash).ok_or_else(|| {
        ProviderError::EventDecode(format!(
            "failed to generate proof for message {}",
            message_id
        ))
    })?;

    Ok(Some(ProofResponse {
        root_hash: tree.root_hash,
        root_proof: tree.proof.clone(),
        index: proof.path,
        leaf: proof.leaf, // Now correctly contains leaf_hash
        siblings: proof.siblings,
        original_list: tree.leaf_hashes.clone(),
    }))
}

/// Verify a merkle proof
pub fn verify_merkle_proof(proof: &ProofResponse) -> bool {
    let merkle_proof = crate::crypto::MerkleProof {
        leaf: proof.leaf,
        siblings: proof.siblings.clone(),
        path: proof.index,
    };
    crate::crypto::verify_proof(&merkle_proof, proof.root_hash)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::storage::{MerkleTreeData, MessageData, MessageMetadata, MessageStatus};
    use alloy::primitives::B256;
    use tempfile::tempdir;

    fn test_storage() -> (Storage, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        (storage, dir)
    }

    fn test_message(id: B256) -> MessageData {
        MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 12345,
                message_id: id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: b"test data".to_vec(),
        }
    }

    #[test]
    fn test_generate_proof_response_message_not_found() {
        let (storage, _dir) = test_storage();
        let msg_id = B256::from_slice(&[0x01u8; 32]);

        let result = generate_proof_response(&storage, &msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_proof_response_no_merkle_tree() {
        let (storage, _dir) = test_storage();
        let msg_id = B256::from_slice(&[0x01u8; 32]);

        // Save message but no merkle tree
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let result = generate_proof_response(&storage, &msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_proof_response_success() {
        let (storage, _dir) = test_storage();
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let leaf_hash = alloy::primitives::keccak256(msg_id.as_slice());
        let root_hash = B256::from_slice(&[0xAAu8; 32]);

        // Save message
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();
        storage.update_message_status(&msg_id, MessageStatus::Signed).unwrap();

        // Create merkle tree with single message (requires zero padding)
        let leaf_hash_b256 = B256::from_slice(leaf_hash.as_slice());
        let mut leaves = vec![leaf_hash_b256, B256::ZERO];
        leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![msg_id],
            leaf_hashes: leaves,
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![12345],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        let result = generate_proof_response(&storage, &msg_id).unwrap();
        assert!(result.is_some());
        let proof = result.unwrap();
        assert_eq!(proof.root_hash, root_hash);
    }

    #[test]
    fn test_verify_merkle_proof_valid() {
        // Create a minimal valid proof
        let leaf = B256::from_slice(&[0x01u8; 32]);
        let sibling = B256::from_slice(&[0x02u8; 32]);

        // Compute expected root
        let mut sorted = vec![leaf, sibling];
        sorted.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        let root = crate::crypto::merkle_root(&sorted).unwrap();

        let proof = ProofResponse {
            root_hash: root,
            root_proof: vec![],
            index: if leaf.as_slice() < sibling.as_slice() { 0 } else { 1 },
            leaf,
            siblings: vec![sibling],
            original_list: sorted,
        };

        assert!(verify_merkle_proof(&proof));
    }

    #[test]
    fn test_verify_merkle_proof_invalid() {
        let proof = ProofResponse {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            root_proof: vec![],
            index: 0,
            leaf: B256::from_slice(&[0x01u8; 32]),
            siblings: vec![B256::from_slice(&[0x02u8; 32])],
            original_list: vec![],
        };

        // This should fail as the root doesn't match
        assert!(!verify_merkle_proof(&proof));
    }

    fn minimal_app_config(provider: &str) -> crate::config::AppConfig {
        use crate::config::*;
        use crate::provider::types::LayerZeroConfig;
        use std::time::Duration;
        use std::collections::HashMap;

        AppConfig {
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
            provider: provider.to_string(),
            layerzero: Some(LayerZeroConfig {
                eid_to_chain_id: {
                    let mut map = HashMap::new();
                    map.insert(40232, 31338);
                    map
                },
                dvn_addresses: {
                    let mut map = HashMap::new();
                    map.insert(31338, "0x1234567890123456789012345678901234567890".to_string());
                    map
                },
            }),
            chainlink_ccv: None,
        }
    }

    #[test]
    fn test_create_provider_layerzero() {
        let (storage, _dir) = test_storage();
        let storage = Arc::new(storage);

        let config = Arc::new(minimal_app_config("layerzero"));
        let provider = create_provider(Arc::clone(&config), Arc::clone(&storage));

        assert!(provider.is_ok());
        let provider = provider.unwrap();
        assert_eq!(provider.name(), "layerzero");
    }

    #[test]
    fn test_create_provider_unknown() {
        let (storage, _dir) = test_storage();
        let storage = Arc::new(storage);

        let config = Arc::new(minimal_app_config("unknown_provider"));

        let provider = create_provider(Arc::clone(&config), Arc::clone(&storage));
        assert!(provider.is_err());
        match provider {
            Err(ProviderError::UnknownEvent(_)) => {}
            _ => panic!("expected UnknownEvent error"),
        }
    }
}
