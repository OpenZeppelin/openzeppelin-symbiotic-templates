//! Relay Submitter Job
//!
//! Submits signed proofs to destination chains via OpenZeppelin Relayer.
//! This replaces direct EVM signing with OZ Relayer's transaction management.

use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::B256;
use tokio::sync::broadcast;

use crate::config::AppConfig;
use crate::crypto::generate_proof;
use crate::error::RelayerError;
use crate::provider::DynProvider;
use crate::relayer_client::{EvmTransactionRequest, RelayerClient, Speed, TransactionStatus};
use crate::storage::{MerkleTreeData, Storage, SubmissionState, SubmissionStatus};

/// RelaySubmitterJob submits signed proofs to destination chains via OZ Relayer
pub struct RelaySubmitterJob {
    storage: Arc<Storage>,
    provider: DynProvider,
    config: Arc<AppConfig>,
    relayer_client: RelayerClient,
}

impl RelaySubmitterJob {
    /// Create a new relay submitter job
    pub fn new(
        storage: Arc<Storage>,
        provider: DynProvider,
        config: Arc<AppConfig>,
        relayer_client: RelayerClient,
    ) -> Self {
        Self {
            storage,
            provider,
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
        let provider_clone = Arc::clone(&self.provider);
        let config_clone = Arc::clone(&self.config);
        let client_clone = self.relayer_client.clone();

        let submit_handle = tokio::spawn(async move {
            Self::run_submission_loop(
                storage_clone,
                provider_clone,
                config_clone,
                client_clone,
                shutdown_rx_submit,
            )
            .await
        });

        // Spawn status polling loop (fallback for missed webhooks)
        let storage_clone = Arc::clone(&self.storage);
        let config_clone = Arc::clone(&self.config);
        let client_clone = self.relayer_client.clone();

        let status_handle = tokio::spawn(async move {
            Self::run_status_poll_loop(
                storage_clone,
                config_clone,
                client_clone,
                shutdown_rx_status,
            )
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
        provider: DynProvider,
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
                    if let Err(e) = Self::process_pending_submissions(&storage, &provider, &config, &client).await {
                        tracing::error!(error = %e, "error processing submissions");
                    }
                }
            }
        }
    }

    /// Process signed trees that need submission
    async fn process_pending_submissions(
        storage: &Storage,
        provider: &DynProvider,
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

            // One on-chain submission covers every message sharing a leaf, so
            // submit once per unique leaf and mark the rest Deduplicated.
            let (primaries, shadows) = Self::partition_by_leaf(storage, provider, &tree);

            if !shadows.is_empty() {
                tracing::warn!(
                    root = %tree.root_hash,
                    total = tree.message_ids.len(),
                    primaries = primaries.len(),
                    shadows = shadows.len(),
                    "batch contains duplicate-leaf messages; submitting once per unique leaf"
                );
            }

            for (shadow_id, primary_id) in &shadows {
                if let Err(e) = Self::record_deduplicated(
                    storage,
                    tree.destination_chain,
                    tree.root_hash,
                    *shadow_id,
                    *primary_id,
                ) {
                    tracing::error!(
                        shadow_id = %shadow_id,
                        primary_id = %primary_id,
                        error = %e,
                        "failed to record deduplicated submission"
                    );
                }
            }

            for message_id in &primaries {
                if let Err(e) = Self::submit_single_message(
                    storage,
                    provider,
                    config,
                    client,
                    &tree,
                    *message_id,
                    &chain_config.target_address,
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

    /// Partition a tree's messages into primaries (one per unique leaf) and
    /// shadows mapped to their primary's id. Unloadable or unhashable rows
    /// fall back to the primaries list so the regular retry path handles them.
    fn partition_by_leaf(
        storage: &Storage,
        provider: &DynProvider,
        tree: &MerkleTreeData,
    ) -> (Vec<B256>, Vec<(B256, B256)>) {
        let mut primaries: Vec<B256> = Vec::new();
        let mut shadows: Vec<(B256, B256)> = Vec::new();
        let mut primary_for_leaf: HashMap<B256, B256> = HashMap::new();

        for msg_id in &tree.message_ids {
            let message = match storage.get_message(msg_id) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    tracing::warn!(
                        message_id = %msg_id,
                        "message not found while partitioning batch; falling back to direct submission"
                    );
                    primaries.push(*msg_id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        message_id = %msg_id,
                        error = %e,
                        "failed to load message while partitioning batch; falling back to direct submission"
                    );
                    primaries.push(*msg_id);
                    continue;
                }
            };

            match provider.compute_leaf_hash(&message) {
                Ok(leaf) => match primary_for_leaf.get(&leaf) {
                    Some(&primary) => shadows.push((*msg_id, primary)),
                    None => {
                        primary_for_leaf.insert(leaf, *msg_id);
                        primaries.push(*msg_id);
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        message_id = %msg_id,
                        error = %e,
                        "failed to compute leaf hash while partitioning batch; falling back to direct submission"
                    );
                    primaries.push(*msg_id);
                }
            }
        }

        (primaries, shadows)
    }

    /// Write a Deduplicated status for a shadow message, but never overwrite
    /// terminal state written by an earlier submission cycle.
    fn record_deduplicated(
        storage: &Storage,
        chain_id: u64,
        root_hash: B256,
        shadow_id: B256,
        primary_id: B256,
    ) -> Result<(), RelayerError> {
        if let Some(existing) = storage.get_submission_status(chain_id, &shadow_id)?
            && existing.status != SubmissionState::Pending
        {
            return Ok(());
        }

        let mut status = SubmissionStatus::new_pending(shadow_id, root_hash, chain_id);
        status.mark_deduplicated(primary_id);
        storage.save_submission_status(&status)?;

        tracing::debug!(
            shadow_id = %shadow_id,
            primary_id = %primary_id,
            chain_id,
            "recorded deduplicated submission"
        );
        Ok(())
    }

    /// Submit a single message proof via OZ Relayer
    async fn submit_single_message(
        storage: &Storage,
        provider: &DynProvider,
        config: &AppConfig,
        client: &RelayerClient,
        tree: &MerkleTreeData,
        message_id: B256,
        target_address: &str,
    ) -> Result<(), RelayerError> {
        let chain_id = tree.destination_chain;

        // Generate idempotency key
        let idem_key = Self::idempotency_key(provider.name(), &message_id, &tree.root_hash);

        // Check if an entry with this idempotency key already exists.
        // Skip only when it has progressed past the pre-submit stage; otherwise retry.
        if let Some(existing) = storage.get_submission_by_idempotency_key(&idem_key)? {
            if existing.relayer_tx_id.is_some() || existing.status != SubmissionState::Pending {
                tracing::debug!(
                    message_id = %message_id,
                    idempotency_key = %idem_key,
                    status = ?existing.status,
                    relayer_tx_id = ?existing.relayer_tx_id,
                    "submission already in progress, skipping"
                );
                return Ok(());
            }

            tracing::warn!(
                message_id = %message_id,
                idempotency_key = %idem_key,
                "found stale pending submission without relayer tx id, retrying"
            );
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

        if tree.epoch.is_none() {
            return Err(RelayerError::EpochMissing);
        }

        // Get message data
        let message = storage
            .get_message(&message_id)?
            .ok_or(RelayerError::MessageNotFound(message_id))?;

        let leaf_hash = provider
            .compute_leaf_hash(&message)
            .map_err(|e| RelayerError::ProofGeneration(e.to_string()))?;

        // Generate merkle proof (siblings)
        let proof = generate_proof(&tree.leaf_hashes, leaf_hash).ok_or_else(|| {
            RelayerError::ProofGeneration("failed to generate merkle proof".into())
        })?;

        let submission = provider
            .prepare_submission(&message, tree, &proof, target_address)
            .map_err(|e| RelayerError::ProofGeneration(e.to_string()))?;

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
        let mut request = EvmTransactionRequest::new(
            submission.to.clone(),
            format!("0x{}", hex::encode(&submission.calldata)),
            speed,
        )
        .with_idempotency_key(idem_key);
        if let Some(gas_limit) = submission.gas_limit {
            request = request.with_gas_limit(gas_limit);
        }

        tracing::info!(
            message_id = %message_id,
            chain_id,
            target = %submission.to,
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

        tracing::debug!(
            count = pending.len(),
            "polling status for pending submissions"
        );

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
                let tx_hash = response
                    .hash
                    .as_ref()
                    .and_then(|h| h.strip_prefix("0x").unwrap_or(h).parse::<B256>().ok());
                status.mark_confirmed(tx_hash);
            }
            TransactionStatus::Failed
            | TransactionStatus::Canceled
            | TransactionStatus::Expired => {
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
    fn idempotency_key(provider: &str, message_id: &B256, root_hash: &B256) -> String {
        format!(
            "bg-{}-{}-{}",
            provider,
            hex::encode(&message_id.0[..8]),
            hex::encode(&root_hash.0[..8])
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, DatabaseConfig, LoggingConfig, OzRelayerConfig, SecurityConfig, ServerConfig,
        SignerConfig, SymbioticRelayConfig,
    };
    use crate::crypto::MerkleProof;
    use crate::error::ProviderError;
    use crate::evm::DecodedJobAssigned;
    use crate::provider::{DynProvider, PreparedSubmission, Provider};
    use crate::relayer_client::ChainRelayerConfig;
    use crate::storage::MessageData;
    use crate::storage::MessageMetadata;
    use crate::storage::SubmissionStatus;
    use crate::webhook::WebhookEvent;
    use alloy::primitives::B256;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn minimal_config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
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
                acceptance_hooks: Vec::new(),
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338],
            provider: "layerzero".to_string(),
            layerzero: None,
            chainlink_ccv: None,
        })
    }

    struct TestProvider;

    #[async_trait]
    impl Provider for TestProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn handle_webhook_event(&self, _event: &WebhookEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn compute_leaf_hash(&self, message: &MessageData) -> Result<B256, ProviderError> {
            let decoded: DecodedJobAssigned = serde_json::from_slice(&message.data)?;
            Ok(crate::crypto::compute_dvn_leaf(
                &decoded.packet_header,
                decoded.payload_hash,
                decoded.confirmations,
            ))
        }

        fn prepare_submission(
            &self,
            _message: &MessageData,
            tree: &MerkleTreeData,
            _proof: &MerkleProof,
            target_address: &str,
        ) -> Result<PreparedSubmission, ProviderError> {
            if tree.epoch.is_none() {
                return Err(ProviderError::EventDecode(
                    "missing epoch on signed tree".to_string(),
                ));
            }

            Ok(PreparedSubmission {
                to: target_address.to_string(),
                calldata: vec![0xde, 0xad, 0xbe, 0xef],
                gas_limit: None,
            })
        }
    }

    fn test_provider() -> DynProvider {
        Arc::new(TestProvider)
    }

    fn config_with_relayer(base_url: String) -> Arc<AppConfig> {
        let mut cfg = (*minimal_config()).clone();
        cfg.oz_relayer = OzRelayerConfig {
            base_url,
            poll_interval: Duration::from_secs(1),
            status_poll_interval: Duration::from_secs(1),
            default_speed: "fast".to_string(),
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::from_millis(0),
            chain_relayers: vec![crate::config::ChainRelayerEntry {
                chain_id: 31338,
                relayer_id: "relayer-1".to_string(),
                target_address: "0x1234567890123456789012345678901234567890".to_string(),
            }],
        };
        Arc::new(cfg)
    }

    #[test]
    fn test_idempotency_key() {
        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let root = B256::from_slice(&[0x22u8; 32]);

        let key = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);
        assert!(key.starts_with("bg-"));
        assert!(key.contains("1111111111111111"));
        assert!(key.contains("2222222222222222"));
    }

    #[test]
    fn test_idempotency_key_deterministic() {
        let msg_id = B256::from_slice(&[0xAAu8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        let key1 = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);
        let key2 = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);

        assert_eq!(key1, key2, "Same inputs should produce same key");
    }

    #[test]
    fn test_idempotency_key_unique_per_root() {
        let msg_id = B256::from_slice(&[0xCCu8; 32]);
        let root1 = B256::from_slice(&[0x11u8; 32]);
        let root2 = B256::from_slice(&[0x22u8; 32]);

        let key1 = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root1);
        let key2 = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root2);

        assert_ne!(key1, key2, "Different roots should produce different keys");
    }

    /// Test that we can detect a stale pre-submit record:
    /// Pending status with no relayer tx ID.
    ///
    /// The submitter should retry these records instead of skipping forever.
    #[test]
    fn test_detects_stale_pending_entry_without_relayer_tx() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0xDDu8; 32]);
        let root = B256::from_slice(&[0xEEu8; 32]);
        let chain_id = 42161u64;
        let idem_key = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);

        // First submission creates Pending entry (simulating in-flight submission)
        let status =
            SubmissionStatus::new_pending_with_key(msg_id, root, chain_id, idem_key.clone());
        storage.save_submission_status(&status).unwrap();

        // Stale entry is present
        let existing = storage
            .get_submission_by_idempotency_key(&idem_key)
            .unwrap();
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

        // This is the stale state that should be retried by submitter logic.
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
        let idem_key = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);

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

    // ============ Additional Relay Submitter Tests ============

    #[test]
    fn test_idempotency_key_format() {
        let msg_id = B256::from_slice(&[0xAAu8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        let key = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);

        // Should be "bg-" + provider + "-" + 16 hex chars + "-" + 16 hex chars
        assert!(key.starts_with("bg-"));
        let parts: Vec<&str> = key.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1], "layerzero");
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3].len(), 16);
    }

    #[test]
    fn test_update_status_from_response_confirmed() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: Some(
                "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ),
            status: crate::relayer_client::TransactionStatus::Confirmed,
            nonce: Some(1),
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Confirmed);
        assert!(status.tx_hash.is_some());
    }

    #[test]
    fn test_update_status_from_response_mined() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: Some(
                "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_string(),
            ),
            status: crate::relayer_client::TransactionStatus::Mined,
            nonce: Some(1),
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Confirmed);
    }

    #[test]
    fn test_update_status_from_response_failed() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Failed,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: Some("execution reverted".to_string()),
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
        assert_eq!(status.last_error, Some("execution reverted".to_string()));
    }

    #[test]
    fn test_update_status_from_response_canceled() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Canceled,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: Some("user canceled".to_string()),
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
    }

    #[test]
    fn test_update_status_from_response_expired() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Expired,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
    }

    #[test]
    fn test_update_status_from_response_pending_no_change() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);
        status.set_relayer_tx_id("tx-123".to_string());
        let original_status = status.status;

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Pending,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        // Status should not change for pending
        assert_eq!(status.status, original_status);
    }

    #[test]
    fn test_update_status_from_response_sent_no_change() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);
        status.set_relayer_tx_id("tx-123".to_string());
        let original_status = status.status;

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: Some("0x1234".to_string()),
            status: crate::relayer_client::TransactionStatus::Sent,
            nonce: Some(1),
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        // Status should not change for sent
        assert_eq!(status.status, original_status);
    }

    #[test]
    fn test_update_status_hash_parsing() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        // Hash without 0x prefix
        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: Some(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ),
            status: crate::relayer_client::TransactionStatus::Confirmed,
            nonce: Some(1),
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert!(status.tx_hash.is_some());
    }

    #[test]
    fn test_update_status_invalid_hash() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        // Invalid hash format
        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: Some("invalid-hash".to_string()),
            status: crate::relayer_client::TransactionStatus::Confirmed,
            nonce: Some(1),
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        // Should still be confirmed, but no tx_hash
        assert_eq!(status.status, SubmissionState::Confirmed);
        assert!(status.tx_hash.is_none());
    }

    #[test]
    fn test_submission_status_new_pending_with_key() {
        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let root = B256::from_slice(&[0x22u8; 32]);
        let chain_id = 31338u64;
        let idem_key = "test-key-123".to_string();

        let status =
            SubmissionStatus::new_pending_with_key(msg_id, root, chain_id, idem_key.clone());

        assert_eq!(status.message_id, msg_id);
        assert_eq!(status.root_hash, root);
        assert_eq!(status.destination_chain, chain_id);
        assert_eq!(status.idempotency_key, Some(idem_key));
        assert_eq!(status.status, SubmissionState::Pending);
        assert!(status.relayer_tx_id.is_none());
    }

    #[tokio::test]
    async fn test_submit_single_message_missing_epoch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let config = minimal_config();
        let provider = test_provider();
        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            std::time::Duration::from_secs(1),
            0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };

        let err = RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            B256::from_slice(&[0x01u8; 32]),
            "0x1234567890123456789012345678901234567890",
        )
        .await
        .unwrap_err();

        assert!(matches!(err, RelayerError::EpochMissing));
    }

    #[tokio::test]
    async fn test_submit_single_message_missing_message() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let config = minimal_config();
        let provider = test_provider();
        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            std::time::Duration::from_secs(1),
            0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![B256::from_slice(&[0x01u8; 32])],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: Some(1),
            attested_at: None,
        };

        let err = RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            B256::from_slice(&[0x01u8; 32]),
            "0x1234567890123456789012345678901234567890",
        )
        .await
        .unwrap_err();

        assert!(matches!(err, RelayerError::MessageNotFound(_)));
    }

    #[tokio::test]
    async fn test_process_pending_submissions_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = minimal_config();
        let provider = test_provider();
        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![],
            std::time::Duration::from_secs(1),
            0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();

        let result =
            RelaySubmitterJob::process_pending_submissions(&storage, &provider, &config, &client)
                .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_pending_submissions_chain_not_configured() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = minimal_config();
        let provider = test_provider();
        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                1,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            std::time::Duration::from_secs(1),
            0,
            std::time::Duration::from_millis(0),
        )
        .unwrap();

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![B256::from_slice(&[0x01u8; 32])],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32]), B256::ZERO],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let result =
            RelaySubmitterJob::process_pending_submissions(&storage, &provider, &config, &client)
                .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_submit_single_message_success_updates_status() {
        let server = MockServer::start().await;
        let create_tx_response = serde_json::json!({
            "success": true,
            "data": { "id": "tx-123" },
            "error": null
        });

        Mock::given(method("POST"))
            .and(path("/api/v1/relayers/relayer-1/transactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(create_tx_response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = config_with_relayer(server.uri());

        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x10u8; 32]),
            src_eid: 40231,
            dst_eid: 40232,
            sender: alloy::primitives::Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x03u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 1,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        let msg_id = job.message_id();
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 100,
                message_id: msg_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let leaf = crate::crypto::compute_dvn_leaf(
            &job.packet_header,
            job.payload_hash,
            job.confirmations,
        );
        let mut leaves = vec![leaf, B256::ZERO];
        leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![msg_id],
            leaf_hashes: leaves,
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();
        let provider = test_provider();

        RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            msg_id,
            "0x1234567890123456789012345678901234567890",
        )
        .await
        .unwrap();

        let status = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(status.relayer_tx_id, Some("tx-123".to_string()));
    }

    #[tokio::test]
    async fn test_submit_single_message_retries_stale_pending_entry() {
        let server = MockServer::start().await;
        let create_tx_response = serde_json::json!({
            "success": true,
            "data": { "id": "tx-124" },
            "error": null
        });

        Mock::given(method("POST"))
            .and(path("/api/v1/relayers/relayer-1/transactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(create_tx_response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = config_with_relayer(server.uri());

        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x21u8; 32]),
            src_eid: 40231,
            dst_eid: 40232,
            sender: alloy::primitives::Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x04u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 2,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        let msg_id = job.message_id();
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 101,
                message_id: msg_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let leaf = crate::crypto::compute_dvn_leaf(
            &job.packet_header,
            job.payload_hash,
            job.confirmations,
        );
        let mut leaves = vec![leaf, B256::ZERO];
        leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xABu8; 32]),
            message_ids: vec![msg_id],
            leaf_hashes: leaves,
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };

        // Simulate the stale state: pending entry created, but no relayer tx id persisted.
        let stale_key = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &tree.root_hash);
        let stale_status =
            SubmissionStatus::new_pending_with_key(msg_id, tree.root_hash, 31338, stale_key);
        storage.save_submission_status(&stale_status).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();
        let provider = test_provider();

        RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            msg_id,
            "0x1234567890123456789012345678901234567890",
        )
        .await
        .unwrap();

        let status = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(status.relayer_tx_id, Some("tx-124".to_string()));
        assert_eq!(status.status, crate::storage::SubmissionState::Submitted);
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![],
            Duration::from_secs(1),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let result = RelaySubmitterJob::poll_pending_statuses(&storage, &client).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_transaction_not_found() {
        let server = MockServer::start().await;
        let tx_id = "tx-not-found";

        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/relayers/relayer-1/transactions/{tx_id}"
            )))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "not found"
            })))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x33u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status.set_relayer_tx_id(tx_id.to_string());
        storage.save_submission_status(&status).unwrap();

        // Should not error - TransactionNotFound is handled gracefully
        let result = RelaySubmitterJob::poll_pending_statuses(&storage, &client).await;
        assert!(result.is_ok());

        // Status should remain unchanged (Submitted)
        let updated = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, SubmissionState::Submitted);
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_failed_response() {
        let server = MockServer::start().await;
        let tx_id = "tx-fail";

        let response = serde_json::json!({
            "id": tx_id,
            "hash": null,
            "status": "failed",
            "nonce": null,
            "createdAt": null,
            "sentAt": null,
            "confirmedAt": null,
            "statusReason": "execution reverted"
        });

        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/relayers/relayer-1/transactions/{tx_id}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x44u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status.set_relayer_tx_id(tx_id.to_string());
        storage.save_submission_status(&status).unwrap();

        RelaySubmitterJob::poll_pending_statuses(&storage, &client)
            .await
            .unwrap();

        let updated = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, SubmissionState::Failed);
        assert_eq!(updated.last_error, Some("execution reverted".to_string()));
    }

    #[tokio::test]
    async fn test_submit_single_message_skips_submitted_idempotency() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = minimal_config();
        let provider = test_provider();

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(1),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x55u8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);
        let idem_key = RelaySubmitterJob::idempotency_key("test", &msg_id, &root);

        // Create an entry that already has a relayer_tx_id (should be skipped)
        let mut status = SubmissionStatus::new_pending_with_key(msg_id, root, 31338, idem_key);
        status.set_relayer_tx_id("tx-existing".to_string());
        storage.save_submission_status(&status).unwrap();

        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };

        // Should succeed without hitting the relayer (skipped due to idempotency)
        let result = RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            msg_id,
            "0x1234567890123456789012345678901234567890",
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_submit_single_message_skips_confirmed_status() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = minimal_config();
        let provider = test_provider();

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(1),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x66u8; 32]);
        let root = B256::from_slice(&[0xCCu8; 32]);

        // Create a confirmed status entry (should be skipped via second dedup check)
        let mut status = SubmissionStatus::new_pending(msg_id, root, 31338);
        status.mark_confirmed(None);
        storage.save_submission_status(&status).unwrap();

        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };

        let result = RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            msg_id,
            "0x1234567890123456789012345678901234567890",
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_pending_submissions_with_signed_tree() {
        let server = MockServer::start().await;
        let create_tx_response = serde_json::json!({
            "success": true,
            "data": { "id": "tx-bulk" },
            "error": null
        });

        Mock::given(method("POST"))
            .and(path("/api/v1/relayers/relayer-1/transactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(create_tx_response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = config_with_relayer(server.uri());

        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x70u8; 32]),
            src_eid: 40231,
            dst_eid: 40232,
            sender: alloy::primitives::Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x71u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 3,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        let msg_id = job.message_id();
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 200,
                message_id: msg_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let leaf = crate::crypto::compute_dvn_leaf(
            &job.packet_header,
            job.payload_hash,
            job.confirmations,
        );
        let mut leaves = vec![leaf, B256::ZERO];
        leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0x72u8; 32]),
            message_ids: vec![msg_id],
            leaf_hashes: leaves,
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![200],
            proof: vec![0u8; 96],
            epoch: Some(2),
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();
        let provider = test_provider();

        let result =
            RelaySubmitterJob::process_pending_submissions(&storage, &provider, &config, &client)
                .await;
        assert!(result.is_ok());

        // Verify a submission status was created
        let status = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(status.relayer_tx_id, Some("tx-bulk".to_string()));
    }

    #[test]
    fn test_update_status_from_response_submitted() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);
        status.set_relayer_tx_id("tx-123".to_string());

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Submitted,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        // Submitted is non-terminal, status should remain as-is
        assert_eq!(status.status, SubmissionState::Submitted);
    }

    #[test]
    fn test_update_status_confirmed_no_hash() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Confirmed,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Confirmed);
        assert!(status.tx_hash.is_none());
    }

    #[test]
    fn test_update_status_failed_without_reason() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Failed,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: None,
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_api_error() {
        let server = MockServer::start().await;
        let tx_id = "tx-err";

        // Server returns 500 error
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/relayers/relayer-1/transactions/{tx_id}"
            )))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x77u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status.set_relayer_tx_id(tx_id.to_string());
        storage.save_submission_status(&status).unwrap();

        // Should not error (API errors are logged but not propagated)
        let result = RelaySubmitterJob::poll_pending_statuses(&storage, &client).await;
        assert!(result.is_ok());

        // Status should remain unchanged
        let updated = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, SubmissionState::Submitted);
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_mined_status() {
        let server = MockServer::start().await;
        let tx_id = "tx-mined";
        let response = serde_json::json!({
            "id": tx_id,
            "hash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "status": "mined",
            "nonce": 5,
            "createdAt": null,
            "sentAt": null,
            "confirmedAt": null,
            "statusReason": null
        });

        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/relayers/relayer-1/transactions/{tx_id}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x88u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status.set_relayer_tx_id(tx_id.to_string());
        storage.save_submission_status(&status).unwrap();

        RelaySubmitterJob::poll_pending_statuses(&storage, &client)
            .await
            .unwrap();

        let updated = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, SubmissionState::Confirmed);
        assert!(updated.tx_hash.is_some());
    }

    #[test]
    fn test_idempotency_key_different_providers() {
        let msg_id = B256::from_slice(&[0xAAu8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        let key1 = RelaySubmitterJob::idempotency_key("layerzero", &msg_id, &root);
        let key2 = RelaySubmitterJob::idempotency_key("chainlink_ccv", &msg_id, &root);

        assert_ne!(
            key1, key2,
            "Different providers should produce different keys"
        );
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_skips_no_relayer_tx_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(1),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        // Create a pending submission without relayer_tx_id (should be skipped)
        let msg_id = B256::from_slice(&[0x99u8; 32]);
        let status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        // Note: new_pending doesn't set relayer_tx_id, but set_relayer_tx_id marks as Submitted.
        // We need a status that is in the pending relayer submissions list but has no tx_id.
        // Actually, list_pending_relayer_submissions returns entries with Submitted status.
        // A Pending entry without relayer_tx_id won't be in that list. Let me check...
        // The poll loop filters entries that have relayer_tx_id = None via the match on line 335-338.
        // So we need an entry with Submitted status but somehow missing relayer_tx_id.
        // Let's create one manually.
        let mut status_submitted = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status_submitted.set_relayer_tx_id("temp".to_string());
        // Now overwrite relayer_tx_id to None after it's Submitted
        status_submitted.relayer_tx_id = None;
        storage.save_submission_status(&status_submitted).unwrap();

        // Should succeed (skips the entry with no relayer_tx_id)
        let result = RelaySubmitterJob::poll_pending_statuses(&storage, &client).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_status_from_response_failed_with_reason() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Canceled,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: Some("user canceled the transaction".to_string()),
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
        assert_eq!(
            status.last_error,
            Some("user canceled the transaction".to_string())
        );
    }

    #[test]
    fn test_update_status_from_response_expired_with_reason() {
        let mut status = SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338);

        let response = crate::relayer_client::TransactionResponse {
            id: "tx-123".to_string(),
            hash: None,
            status: crate::relayer_client::TransactionStatus::Expired,
            nonce: None,
            created_at: None,
            sent_at: None,
            confirmed_at: None,
            status_reason: Some("tx expired after timeout".to_string()),
        };

        RelaySubmitterJob::update_status_from_response(&mut status, &response);

        assert_eq!(status.status, SubmissionState::Failed);
        assert_eq!(
            status.last_error,
            Some("tx expired after timeout".to_string())
        );
    }

    #[tokio::test]
    async fn test_submit_single_message_skips_failed_status() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = minimal_config();
        let provider = test_provider();

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(1),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x67u8; 32]);
        let root = B256::from_slice(&[0xDDu8; 32]);

        // Create a failed status entry (should be skipped via second dedup check)
        let mut status = SubmissionStatus::new_pending(msg_id, root, 31338);
        status.mark_failed();
        storage.save_submission_status(&status).unwrap();

        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96],
            epoch: Some(1),
            attested_at: None,
        };

        let result = RelaySubmitterJob::submit_single_message(
            &storage,
            &provider,
            &config,
            &client,
            &tree,
            msg_id,
            "0x1234567890123456789012345678901234567890",
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_poll_pending_statuses_updates_confirmed() {
        let server = MockServer::start().await;
        let tx_id = "tx-999";
        let response = serde_json::json!({
            "id": tx_id,
            "hash": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "status": "confirmed",
            "nonce": 1,
            "createdAt": null,
            "sentAt": null,
            "confirmedAt": null,
            "statusReason": null
        });

        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/relayers/relayer-1/transactions/{tx_id}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();

        let msg_id = B256::from_slice(&[0x22u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        status.set_relayer_tx_id(tx_id.to_string());
        storage.save_submission_status(&status).unwrap();

        RelaySubmitterJob::poll_pending_statuses(&storage, &client)
            .await
            .unwrap();

        let updated = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, SubmissionState::Confirmed);
        assert!(updated.tx_hash.is_some());
    }

    /// When two messages in a batch hash to the same leaf, the submitter
    /// sends exactly one on-chain transaction and marks the shadow as
    /// Deduplicated.
    #[tokio::test]
    async fn test_process_pending_submissions_dedupes_duplicate_leaves() {
        let server = MockServer::start().await;
        let create_tx_response = serde_json::json!({
            "success": true,
            "data": { "id": "tx-dedup" },
            "error": null
        });

        // `expect(1)` is the key assertion: we want exactly ONE tx, even
        // though the tree contains two message ids sharing a leaf.
        Mock::given(method("POST"))
            .and(path("/api/v1/relayers/relayer-1/transactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(create_tx_response))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        let config = config_with_relayer(server.uri());

        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x80u8; 32]),
            src_eid: 40231,
            dst_eid: 40232,
            sender: alloy::primitives::Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x81u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 7,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        // Two distinct message ids with identical job data → identical leaf.
        let primary_id = B256::from_slice(&[0x01u8; 32]);
        let shadow_id = B256::from_slice(&[0x02u8; 32]);
        for (id, tx_byte) in [(primary_id, 0xAAu8), (shadow_id, 0xBBu8)] {
            let msg = MessageData {
                metadata: MessageMetadata {
                    source_chain: 1,
                    destination_chain: 31338,
                    block_number: 200,
                    message_id: id,
                    event_tx_hash: B256::from_slice(&[tx_byte; 32]),
                    ttl: None,
                },
                data: serde_json::to_vec(&job).unwrap(),
            };
            storage.save_message(&msg).unwrap();
        }

        let leaf = crate::crypto::compute_dvn_leaf(
            &job.packet_header,
            job.payload_hash,
            job.confirmations,
        );

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0x82u8; 32]),
            message_ids: vec![primary_id, shadow_id],
            leaf_hashes: vec![leaf],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![200],
            proof: vec![0u8; 96],
            epoch: Some(2),
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let client = RelayerClient::new(
            server.uri(),
            "test-api-key".to_string(),
            vec![ChainRelayerConfig::new(
                31338,
                "relayer-1".to_string(),
                "0x1234567890123456789012345678901234567890".to_string(),
            )],
            Duration::from_secs(5),
            0,
            Duration::from_millis(0),
        )
        .unwrap();
        let provider = test_provider();

        RelaySubmitterJob::process_pending_submissions(&storage, &provider, &config, &client)
            .await
            .unwrap();

        // Mock server's `expect(1)` is checked when it drops — forcing it now
        // produces a clearer failure if a second request snuck through.
        server.verify().await;

        let primary_status = storage
            .get_submission_status(31338, &primary_id)
            .unwrap()
            .unwrap();
        assert_eq!(primary_status.relayer_tx_id, Some("tx-dedup".to_string()));
        assert_eq!(primary_status.status, SubmissionState::Submitted);

        let shadow_status = storage
            .get_submission_status(31338, &shadow_id)
            .unwrap()
            .unwrap();
        assert_eq!(shadow_status.status, SubmissionState::Deduplicated);
        assert!(shadow_status.relayer_tx_id.is_none());
        assert!(
            shadow_status
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("deduplicated via"))
        );

        let mut final_primary = primary_status.clone();
        final_primary.mark_confirmed(Some(B256::from_slice(&[0xCCu8; 32])));
        storage.save_submission_status(&final_primary).unwrap();

        let remaining = storage.list_signed_trees_without_submissions().unwrap();
        assert!(
            remaining.is_empty(),
            "once the primary is Confirmed and the shadow is Deduplicated, the tree must clear"
        );
    }
}
