use std::sync::Arc;

use alloy::primitives::B256;
use async_trait::async_trait;
use axum::Router;

use crate::api::AppState;
use crate::config::AppConfig;
use crate::crypto::generate_proof;
use crate::error::ProviderError;
use crate::storage::{MessageData, Storage};
use crate::webhook::{ProofResponse, WebhookEvent};

pub mod layerzero;

pub use layerzero::LayerZeroProvider;

/// Type alias for a thread-safe, dynamically-dispatched provider
pub type DynProvider = Arc<dyn Provider>;

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
