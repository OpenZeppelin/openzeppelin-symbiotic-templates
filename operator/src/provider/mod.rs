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
            let ccv_config = config.chainlink_ccv.clone().ok_or_else(|| {
                ProviderError::EventDecode("chainlink_ccv config section is required".to_string())
            })?;
            Ok(Arc::new(ChainlinkCcvProvider::new(
                ccv_config, config, storage,
            )?))
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
    provider: &DynProvider,
    message_id: &B256,
) -> Result<Option<ProofResponse>, ProviderError> {
    let message = match storage.get_message(message_id)? {
        Some(m) => m,
        None => return Ok(None),
    };

    let root_hash = match storage.get_merkle_root_by_message(message_id)? {
        Some(r) => r,
        None => return Ok(None),
    };

    let tree = match storage.get_merkle_tree_by_root(&root_hash)? {
        Some(t) => t,
        None => return Ok(None),
    };

    // Compute the leaf from the message itself rather than positional lookup
    // into tree.message_ids / tree.leaf_hashes. The signer sorts those arrays
    // by different keys and may carry duplicate-leaf shadow ids, so positional
    // alignment is not a reliable invariant.
    let leaf_hash = provider.compute_leaf_hash(&message)?;

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
        leaf: proof.leaf,
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
    use async_trait::async_trait;
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

    /// Test-only provider whose leaf hash is `keccak256(message_id)`. Matches
    /// the fake leaves used by tests that hard-code tree contents.
    struct MsgIdLeafProvider;

    #[async_trait]
    impl Provider for MsgIdLeafProvider {
        fn name(&self) -> &'static str {
            "msgid-leaf"
        }

        async fn handle_webhook_event(&self, _event: &WebhookEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn compute_leaf_hash(&self, message: &MessageData) -> Result<B256, ProviderError> {
            Ok(alloy::primitives::keccak256(
                message.metadata.message_id.as_slice(),
            ))
        }
    }

    fn test_proof_provider() -> DynProvider {
        Arc::new(MsgIdLeafProvider)
    }

    #[test]
    fn test_generate_proof_response_message_not_found() {
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();
        let msg_id = B256::from_slice(&[0x01u8; 32]);

        let result = generate_proof_response(&storage, &provider, &msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_proof_response_no_merkle_tree() {
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();
        let msg_id = B256::from_slice(&[0x01u8; 32]);

        // Save message but no merkle tree
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let result = generate_proof_response(&storage, &provider, &msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_proof_response_success() {
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let leaf_hash = alloy::primitives::keccak256(msg_id.as_slice());
        let root_hash = B256::from_slice(&[0xAAu8; 32]);

        // Save message
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();
        storage
            .update_message_status(&msg_id, MessageStatus::Signed)
            .unwrap();

        let leaf_hash_b256 = B256::from_slice(leaf_hash.as_slice());
        let leaves = vec![leaf_hash_b256];

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

        let result = generate_proof_response(&storage, &provider, &msg_id).unwrap();
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
            index: if leaf.as_slice() < sibling.as_slice() {
                0
            } else {
                1
            },
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
        use std::collections::HashMap;
        use std::time::Duration;

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
                target_addresses: {
                    let mut map = HashMap::new();
                    map.insert(
                        31338,
                        "0x1234567890123456789012345678901234567890".to_string(),
                    );
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
    fn test_create_provider_chainlink_requires_config() {
        let (storage, _dir) = test_storage();
        let storage = Arc::new(storage);

        let config = Arc::new(minimal_app_config("chainlink_ccv"));
        let provider = create_provider(Arc::clone(&config), Arc::clone(&storage));

        assert!(provider.is_err());
        match provider {
            Err(ProviderError::EventDecode(msg)) => {
                assert!(msg.contains("chainlink_ccv config section is required"));
            }
            _ => panic!("expected EventDecode error"),
        }
    }

    #[test]
    fn test_default_trait_methods() {
        use crate::storage::{MerkleTreeData, MessageData, MessageMetadata};

        struct MinimalProvider;

        #[async_trait]
        impl Provider for MinimalProvider {
            fn name(&self) -> &'static str {
                "minimal"
            }

            async fn handle_webhook_event(
                &self,
                _event: &WebhookEvent,
            ) -> Result<(), ProviderError> {
                Ok(())
            }
        }

        let provider = MinimalProvider;

        // max_batch_size default
        assert_eq!(provider.max_batch_size(), usize::MAX);

        // compute_leaf_hash default - should return error
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 2,
                block_number: 1,
                message_id: B256::ZERO,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: vec![],
        };
        assert!(provider.compute_leaf_hash(&msg).is_err());

        // encode_signing_message default - should return error
        let tree = MerkleTreeData {
            root_hash: B256::ZERO,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 2,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };
        assert!(provider.encode_signing_message(&tree).is_err());

        // prepare_submission default - should return error
        let proof = crate::crypto::MerkleProof {
            leaf: B256::ZERO,
            siblings: vec![],
            path: 0,
        };
        assert!(
            provider
                .prepare_submission(&msg, &tree, &proof, "0x0")
                .is_err()
        );
    }

    #[test]
    fn test_prepared_submission_clone() {
        let sub = PreparedSubmission {
            to: "0x1234".to_string(),
            calldata: vec![0xde, 0xad],
        };
        let cloned = sub.clone();
        assert_eq!(cloned.to, sub.to);
        assert_eq!(cloned.calldata, sub.calldata);
    }

    #[test]
    fn test_generate_proof_response_leaf_not_in_tree() {
        // When the message's computed leaf isn't present in tree.leaf_hashes
        // (shouldn't happen in practice, but guard the error path).
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root_hash = B256::from_slice(&[0xAAu8; 32]);

        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();
        storage
            .update_message_status(&msg_id, MessageStatus::Signed)
            .unwrap();

        // Storage indexes msg_id to this tree, but leaf_hashes contains a leaf
        // that doesn't match what the provider computes from the message.
        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![msg_id],
            leaf_hashes: vec![
                B256::from_slice(&[0xFFu8; 32]),
                B256::from_slice(&[0xEEu8; 32]),
            ],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![12345],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        let result = generate_proof_response(&storage, &provider, &msg_id);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("failed to generate proof"),
            "unexpected error: {}",
            err_msg
        );
    }

    #[test]
    fn test_generate_proof_response_message_id_not_in_tree() {
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let other_msg_id = B256::from_slice(&[0x02u8; 32]);
        let root_hash = B256::from_slice(&[0xAAu8; 32]);

        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();
        storage
            .update_message_status(&msg_id, MessageStatus::Signed)
            .unwrap();

        // Tree doesn't index msg_id, so get_merkle_root_by_message returns None.
        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![other_msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xFFu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![12345],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        let result = generate_proof_response(&storage, &provider, &msg_id).unwrap();
        assert!(result.is_none());
    }

    /// Every indexed message must resolve to a valid proof even when
    /// `tree.message_ids` and `tree.leaf_hashes` are sorted by different keys.
    #[test]
    fn test_generate_proof_response_multi_message_mismatched_order() {
        let (storage, _dir) = test_storage();
        let provider = test_proof_provider();

        // Find an id pair whose keccak leaves sort opposite to the ids.
        let (msg1_id, msg2_id, leaf1, leaf2) = {
            let mut found = None;
            for seed in 0u8..64 {
                let a = B256::from_slice(&[seed; 32]);
                let b = B256::from_slice(&[seed.wrapping_add(1); 32]);
                let la = B256::from_slice(alloy::primitives::keccak256(a.as_slice()).as_slice());
                let lb = B256::from_slice(alloy::primitives::keccak256(b.as_slice()).as_slice());
                let id_asc = a.as_slice() < b.as_slice();
                let leaf_asc = la.as_slice() < lb.as_slice();
                if id_asc != leaf_asc {
                    found = Some((a, b, la, lb));
                    break;
                }
            }
            found.expect("failed to find an id pair with disagreeing leaf order")
        };

        // Mirror the signer: message_ids sorted by id, leaf_hashes by leaf.
        let mut sorted_ids = vec![msg1_id, msg2_id];
        sorted_ids.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        let mut sorted_leaves = vec![leaf1, leaf2];
        sorted_leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let root_hash = crate::crypto::merkle_root(&sorted_leaves).unwrap();
        let tree = MerkleTreeData {
            root_hash,
            message_ids: sorted_ids,
            leaf_hashes: sorted_leaves,
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![12345, 12346],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };

        for id in [msg1_id, msg2_id] {
            storage.save_message(&test_message(id)).unwrap();
        }
        storage.save_merkle_tree(&tree).unwrap();

        for id in [msg1_id, msg2_id] {
            let resp = generate_proof_response(&storage, &provider, &id)
                .unwrap_or_else(|e| panic!("unexpected error for {}: {}", id, e))
                .unwrap_or_else(|| panic!("no proof returned for {}", id));
            assert_eq!(resp.root_hash, root_hash);
            // Returned leaf must match what the provider computes from the message.
            let expected_leaf = provider
                .compute_leaf_hash(&test_message(id))
                .unwrap();
            assert_eq!(resp.leaf, expected_leaf, "wrong leaf returned for {}", id);
        }
    }

    #[test]
    fn test_create_provider_layerzero_case_insensitive() {
        let (storage, _dir) = test_storage();
        let storage = Arc::new(storage);

        let config = Arc::new(minimal_app_config("LayerZero"));
        let provider = create_provider(Arc::clone(&config), Arc::clone(&storage));

        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "layerzero");
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
