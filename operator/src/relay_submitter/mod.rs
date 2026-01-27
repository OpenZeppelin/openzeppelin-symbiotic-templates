//! Relay Submitter Job
//!
//! Submits signed proofs to destination chains via OpenZeppelin Relayer.
//! This replaces direct EVM signing with OZ Relayer's transaction management.

use std::sync::Arc;

use alloy::primitives::B256;
use tokio::sync::broadcast;

use crate::config::AppConfig;
use crate::crypto::{compute_dvn_leaf, generate_proof};
use crate::error::RelayerError;
use crate::evm::DecodedJobAssigned;
use crate::relayer_client::{EvmTransactionRequest, RelayerClient, Speed, TransactionStatus};
use crate::storage::{MerkleTreeData, Storage, SubmissionState, SubmissionStatus};
use crate::submitter::dvn::{build_signature, encode_submit_proof};

/// RelaySubmitterJob submits signed proofs to destination chains via OZ Relayer
pub struct RelaySubmitterJob {
    storage: Arc<Storage>,
    config: Arc<AppConfig>,
    relayer_client: RelayerClient,
}

impl RelaySubmitterJob {
    /// Create a new relay submitter job
    pub fn new(
        storage: Arc<Storage>,
        config: Arc<AppConfig>,
        relayer_client: RelayerClient,
    ) -> Self {
        Self {
            storage,
            config,
            relayer_client,
        }
    }

    /// Run the relay submitter job
    pub async fn run(self, shutdown_rx: broadcast::Receiver<()>) -> Result<(), RelayerError> {
        // Create shutdown receivers for each loop
        let shutdown_rx_submit = shutdown_rx.resubscribe();
        let shutdown_rx_status = shutdown_rx.resubscribe();

        // Spawn submission loop
        let storage_clone = Arc::clone(&self.storage);
        let config_clone = Arc::clone(&self.config);
        let client_clone = self.relayer_client.clone();

        let submit_handle = tokio::spawn(async move {
            Self::run_submission_loop(storage_clone, config_clone, client_clone, shutdown_rx_submit)
                .await
        });

        // Spawn status polling loop (fallback for missed webhooks)
        let storage_clone = Arc::clone(&self.storage);
        let config_clone = Arc::clone(&self.config);
        let client_clone = self.relayer_client.clone();

        let status_handle = tokio::spawn(async move {
            Self::run_status_poll_loop(storage_clone, config_clone, client_clone, shutdown_rx_status)
                .await
        });

        // Wait for either loop to complete (shutdown or error)
        tokio::select! {
            result = submit_handle => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "submission loop failed");
                }
            }
            result = status_handle => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "status poll loop failed");
                }
            }
        }

        tracing::info!("relay submitter job shutting down");
        Ok(())
    }

    /// Main submission loop - finds signed trees and submits proofs
    async fn run_submission_loop(
        storage: Arc<Storage>,
        config: Arc<AppConfig>,
        client: RelayerClient,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(config.oz_relayer.poll_interval);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("submission loop shutting down");
                    return;
                }
                _ = interval.tick() => {
                    if let Err(e) = Self::process_pending_submissions(&storage, &config, &client).await {
                        tracing::error!(error = %e, "error processing submissions");
                    }
                }
            }
        }
    }

    /// Process signed trees that need submission
    async fn process_pending_submissions(
        storage: &Storage,
        config: &AppConfig,
        client: &RelayerClient,
    ) -> Result<(), RelayerError> {
        let signed_trees = storage.list_signed_trees_without_submissions()?;

        if signed_trees.is_empty() {
            tracing::debug!("no signed trees need submission");
            return Ok(());
        }

        tracing::info!(count = signed_trees.len(), "found signed trees to submit");

        for tree in signed_trees {
            // Check if this chain is configured in OZ Relayer
            let Some(chain_config) = client.get_chain_config(tree.destination_chain) else {
                tracing::warn!(
                    chain_id = tree.destination_chain,
                    "no OZ Relayer configured for destination chain"
                );
                continue;
            };

            for message_id in tree.message_ids.iter() {
                if let Err(e) = Self::submit_single_message(
                    storage,
                    config,
                    client,
                    &tree,
                    *message_id,
                    &chain_config.dvn_address,
                )
                .await
                {
                    tracing::error!(
                        message_id = %message_id,
                        error = %e,
                        "failed to submit message"
                    );
                }
            }
        }

        Ok(())
    }

    /// Submit a single message proof via OZ Relayer
    async fn submit_single_message(
        storage: &Storage,
        config: &AppConfig,
        client: &RelayerClient,
        tree: &MerkleTreeData,
        message_id: B256,
        dvn_address: &str,
    ) -> Result<(), RelayerError> {
        let chain_id = tree.destination_chain;

        // Generate idempotency key
        let idem_key = Self::idempotency_key(&message_id, &tree.root_hash);

        // Check if we already have a submission in progress (any existing entry = skip)
        // This catches the race window where status is Pending but submission is in-flight
        if storage.get_submission_by_idempotency_key(&idem_key)?.is_some() {
            tracing::debug!(
                message_id = %message_id,
                idempotency_key = %idem_key,
                "submission already in progress, skipping"
            );
            return Ok(());
        }

        // Check if already has a non-pending status (Submitted, Confirmed, or Failed)
        // Any of these states means the submission has already been processed
        if let Some(status) = storage.get_submission_status(chain_id, &message_id)?
            && status.status != SubmissionState::Pending
        {
            tracing::debug!(
                message_id = %message_id,
                status = ?status.status,
                "already processed, skipping"
            );
            return Ok(());
        }

        // Get epoch - this is critical, fail if missing
        let epoch = tree.epoch.ok_or(RelayerError::EpochMissing)?;

        // Get message data
        let message = storage
            .get_message(&message_id)?
            .ok_or(RelayerError::MessageNotFound(message_id))?;

        // Deserialize job assigned data
        let job_assigned: DecodedJobAssigned = serde_json::from_slice(&message.data)?;

        // Compute the leaf hash from message data (not from parallel array index)
        // This is the DVN-compatible leaf hash: keccak256(keccak256(header) || payloadHash || confirmations)
        let leaf_hash = compute_dvn_leaf(
            &job_assigned.packet_header,
            job_assigned.payload_hash,
            job_assigned.confirmations,
        );

        // Generate merkle proof (siblings)
        let proof = generate_proof(&tree.leaf_hashes, leaf_hash).ok_or_else(|| {
            RelayerError::ProofGeneration("failed to generate merkle proof".into())
        })?;

        // Build DVN signature (epoch prefix + BLS proof)
        let signature = build_signature(epoch, &tree.proof);

        // Encode submitProof calldata
        let calldata = encode_submit_proof(
            &job_assigned.packet_header,
            job_assigned.payload_hash,
            job_assigned.confirmations,
            proof.siblings,
            tree.root_hash,
            signature,
        );

        // Store pending status with idempotency key BEFORE submitting
        let mut status = SubmissionStatus::new_pending_with_key(
            message_id,
            tree.root_hash,
            chain_id,
            idem_key.clone(),
        );
        storage.save_submission_status(&status)?;

        // Determine speed
        let speed: Speed = config.oz_relayer.default_speed.parse().unwrap_or_else(|_| {
            tracing::warn!(
                configured_speed = %config.oz_relayer.default_speed,
                "invalid speed in config, defaulting to Fast"
            );
            Speed::Fast
        });

        // Build transaction request
        let request = EvmTransactionRequest::new(
            dvn_address.to_string(),
            format!("0x{}", hex::encode(&calldata)),
            speed,
        )
        .with_idempotency_key(idem_key);

        tracing::info!(
            message_id = %message_id,
            chain_id,
            dvn = %dvn_address,
            "submitting proof to OZ Relayer"
        );

        // Submit to OZ Relayer
        let response = client.send_transaction(chain_id, request).await?;

        // Update status with relayer tx ID
        status.set_relayer_tx_id(response.data.id.clone());
        storage.save_submission_status(&status)?;

        tracing::info!(
            message_id = %message_id,
            relayer_tx_id = %response.data.id,
            "proof submitted to OZ Relayer"
        );

        Ok(())
    }

    /// Status polling loop - fallback for missed webhooks
    async fn run_status_poll_loop(
        storage: Arc<Storage>,
        config: Arc<AppConfig>,
        client: RelayerClient,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(config.oz_relayer.status_poll_interval);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("status poll loop shutting down");
                    return;
                }
                _ = interval.tick() => {
                    if let Err(e) = Self::poll_pending_statuses(&storage, &client).await {
                        tracing::error!(error = %e, "error polling statuses");
                    }
                }
            }
        }
    }

    /// Poll OZ Relayer for status updates on pending submissions
    async fn poll_pending_statuses(
        storage: &Storage,
        client: &RelayerClient,
    ) -> Result<(), RelayerError> {
        let pending = storage.list_pending_relayer_submissions()?;

        if pending.is_empty() {
            tracing::debug!("no pending submissions to poll");
            return Ok(());
        }

        tracing::debug!(count = pending.len(), "polling status for pending submissions");

        for mut status in pending {
            let tx_id = match &status.relayer_tx_id {
                Some(id) => id.clone(),
                None => continue,
            };

            match client
                .get_transaction(status.destination_chain, &tx_id)
                .await
            {
                Ok(tx_response) => {
                    Self::update_status_from_response(&mut status, &tx_response);
                    storage.save_submission_status(&status)?;

                    if tx_response.status.is_terminal() {
                        tracing::info!(
                            message_id = %status.message_id,
                            relayer_tx_id = %tx_id,
                            status = ?tx_response.status,
                            "submission status updated"
                        );
                    }
                }
                Err(RelayerError::TransactionNotFound(_)) => {
                    tracing::warn!(
                        message_id = %status.message_id,
                        relayer_tx_id = %tx_id,
                        "transaction not found in OZ Relayer"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        message_id = %status.message_id,
                        relayer_tx_id = %tx_id,
                        error = %e,
                        "failed to poll transaction status"
                    );
                }
            }
        }

        Ok(())
    }

    /// Update submission status from OZ Relayer transaction response
    fn update_status_from_response(
        status: &mut SubmissionStatus,
        response: &crate::relayer_client::TransactionResponse,
    ) {
        match response.status {
            TransactionStatus::Confirmed | TransactionStatus::Mined => {
                let tx_hash = response.hash.as_ref().and_then(|h| {
                    h.strip_prefix("0x")
                        .unwrap_or(h)
                        .parse::<B256>()
                        .ok()
                });
                status.mark_confirmed(tx_hash);
            }
            TransactionStatus::Failed | TransactionStatus::Canceled | TransactionStatus::Expired => {
                status.mark_failed();
                if let Some(ref reason) = response.status_reason {
                    status.last_error = Some(reason.clone());
                }
            }
            _ => {
                // Still pending/sent/submitted - no update needed
            }
        }
    }

    /// Generate deterministic idempotency key for submission
    fn idempotency_key(message_id: &B256, root_hash: &B256) -> String {
        format!(
            "bg-{}-{}",
            hex::encode(&message_id.0[..8]),
            hex::encode(&root_hash.0[..8])
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::storage::SubmissionStatus;
    use tempfile::tempdir;

    #[test]
    fn test_idempotency_key() {
        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let root = B256::from_slice(&[0x22u8; 32]);

        let key = RelaySubmitterJob::idempotency_key(&msg_id, &root);
        assert!(key.starts_with("bg-"));
        assert!(key.contains("1111111111111111"));
        assert!(key.contains("2222222222222222"));
    }

    #[test]
    fn test_idempotency_key_deterministic() {
        let msg_id = B256::from_slice(&[0xAAu8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        let key1 = RelaySubmitterJob::idempotency_key(&msg_id, &root);
        let key2 = RelaySubmitterJob::idempotency_key(&msg_id, &root);

        assert_eq!(key1, key2, "Same inputs should produce same key");
    }

    #[test]
    fn test_idempotency_key_unique_per_root() {
        let msg_id = B256::from_slice(&[0xCCu8; 32]);
        let root1 = B256::from_slice(&[0x11u8; 32]);
        let root2 = B256::from_slice(&[0x22u8; 32]);

        let key1 = RelaySubmitterJob::idempotency_key(&msg_id, &root1);
        let key2 = RelaySubmitterJob::idempotency_key(&msg_id, &root2);

        assert_ne!(key1, key2, "Different roots should produce different keys");
    }

    /// Test that the deduplication logic skips when ANY idempotency entry exists,
    /// not just entries with relayer_tx_id set.
    ///
    /// This prevents race conditions where two concurrent submissions both pass
    /// the check before either sets relayer_tx_id.
    #[test]
    fn test_dedup_skips_existing_pending_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0xDDu8; 32]);
        let root = B256::from_slice(&[0xEEu8; 32]);
        let chain_id = 42161u64;
        let idem_key = RelaySubmitterJob::idempotency_key(&msg_id, &root);

        // First submission creates Pending entry (simulating in-flight submission)
        let status = SubmissionStatus::new_pending_with_key(
            msg_id,
            root,
            chain_id,
            idem_key.clone(),
        );
        storage.save_submission_status(&status).unwrap();

        // Deduplication check: any existing entry should trigger skip
        let existing = storage.get_submission_by_idempotency_key(&idem_key).unwrap();
        assert!(
            existing.is_some(),
            "Entry with Pending status should be found"
        );

        // The entry has no relayer_tx_id yet (submission in progress)
        let entry = existing.unwrap();
        assert!(
            entry.relayer_tx_id.is_none(),
            "Entry should not have relayer_tx_id yet"
        );

        // This is the fix: we skip based on entry existence, not relayer_tx_id
        // Old buggy code: existing.relayer_tx_id.is_some() -> would NOT skip
        // Fixed code: existing.is_some() -> WILL skip
    }

    /// Test that all non-Pending states trigger the second deduplication check.
    ///
    /// This ensures we don't re-submit messages that are already:
    /// - Submitted (sent to OZ Relayer, awaiting confirmation)
    /// - Confirmed (on-chain, terminal)
    /// - Failed (terminal)
    #[test]
    fn test_dedup_skips_all_non_pending_states() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let chain_id = 42161u64;

        // Test: Submitted state should be skipped
        let msg_submitted = B256::from_slice(&[0x01u8; 32]);
        let mut status1 = SubmissionStatus::new_pending(msg_submitted, B256::ZERO, chain_id);
        status1.set_relayer_tx_id("tx-1".to_string());
        storage.save_submission_status(&status1).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_submitted)
            .unwrap()
            .unwrap();
        assert!(
            retrieved.status != SubmissionState::Pending,
            "Submitted state should trigger skip (status != Pending)"
        );

        // Test: Confirmed state should be skipped
        let msg_confirmed = B256::from_slice(&[0x02u8; 32]);
        let mut status2 = SubmissionStatus::new_pending(msg_confirmed, B256::ZERO, chain_id);
        status2.mark_confirmed(None);
        storage.save_submission_status(&status2).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_confirmed)
            .unwrap()
            .unwrap();
        assert!(
            retrieved.status != SubmissionState::Pending,
            "Confirmed state should trigger skip"
        );

        // Test: Failed state should be skipped
        let msg_failed = B256::from_slice(&[0x03u8; 32]);
        let mut status3 = SubmissionStatus::new_pending(msg_failed, B256::ZERO, chain_id);
        status3.mark_failed();
        storage.save_submission_status(&status3).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_failed)
            .unwrap()
            .unwrap();
        assert!(
            retrieved.status != SubmissionState::Pending,
            "Failed state should trigger skip"
        );
    }

    /// Test the deduplication check order: idempotency key first, then status.
    /// Both checks must pass for submission to proceed.
    #[test]
    fn test_dedup_check_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0xFFu8; 32]);
        let root = B256::from_slice(&[0xAAu8; 32]);
        let chain_id = 42161u64;
        let idem_key = RelaySubmitterJob::idempotency_key(&msg_id, &root);

        // Check 1: No idempotency entry exists -> proceed
        assert!(
            storage
                .get_submission_by_idempotency_key(&idem_key)
                .unwrap()
                .is_none(),
            "Check 1 passes: no idempotency entry"
        );

        // Check 2: No status exists -> proceed (status == None, not Pending)
        assert!(
            storage
                .get_submission_status(chain_id, &msg_id)
                .unwrap()
                .is_none(),
            "Check 2 passes: no status entry"
        );

        // Both checks pass -> submission would proceed
        // (actual submission requires full dependencies, tested via integration tests)
    }
}
