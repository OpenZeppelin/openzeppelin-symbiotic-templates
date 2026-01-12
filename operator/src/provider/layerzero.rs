use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::B256;
use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};

use super::{generate_proof_response, verify_merkle_proof, Provider};
use crate::api::AppState;
use crate::config::{AppConfig, LayerZeroConfig};
use crate::error::ProviderError;
use crate::evm::{job_assigned_topic, DecodedJobAssigned};
use crate::storage::{MessageData, MessageMetadata, Storage};
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

        for log in &event.monitor_match.evm.logs {
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
                        .monitor_match
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
