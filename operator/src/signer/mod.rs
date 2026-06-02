use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::B256;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::acceptance::{
    AcceptanceContext, AcceptanceDecision, AcceptanceHookConfig, evaluate_webhook,
};
use crate::config::AppConfig;
use crate::crypto::merkle_root;
use crate::error::SignerError;
use crate::provider::DynProvider;
use crate::storage::{MerkleTreeData, MessageData, MessageHookState, MessageStatus, Storage};
use crate::symbiotic_relay::SymbioticRelayClientEnum;

/// Merkle root work item
#[derive(Debug, Clone)]
pub struct MerkleRootWorkItem {
    pub root_hash: B256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptanceEvaluation {
    Accepted,
    Deferred,
    Rejected,
}

fn current_unix_timestamp() -> u64 {
    datetime_to_unix_seconds(Utc::now())
}

fn datetime_to_unix_seconds(datetime: DateTime<Utc>) -> u64 {
    let timestamp = datetime.timestamp();
    if timestamp <= 0 { 0 } else { timestamp as u64 }
}

fn acceptance_hook_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static acceptance hook client configuration must be valid")
}

/// Signer job that manages merkle tree creation and signing
pub struct SignerJob {
    storage: Arc<Storage>,
    provider: DynProvider,
    config: Arc<AppConfig>,
    hook_client: reqwest::Client,
}

impl SignerJob {
    /// Create a new signer job
    pub fn new(storage: Arc<Storage>, provider: DynProvider, config: Arc<AppConfig>) -> Self {
        Self {
            storage,
            provider,
            config,
            hook_client: acceptance_hook_client(),
        }
    }

    /// Run the signer job
    pub async fn run(
        self,
        symbiotic_relay_client: SymbioticRelayClientEnum,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), SignerError> {
        // Create work queue channel
        let (tx, rx) = mpsc::channel::<MerkleRootWorkItem>(1000);

        // Clone for workers
        let key_tag = self.config.symbiotic_relay.key_tag as u32;

        // Create shutdown receivers for each task BEFORE spawning
        let shutdown_rx_msg = shutdown_rx.resubscribe();
        let shutdown_rx_sync = shutdown_rx.resubscribe();

        // Start signature processing workers
        let worker_count = self.config.signer.sign_worker_count;
        tracing::info!(
            num_workers = worker_count,
            "starting signature processing workers"
        );

        // Message processing loop
        let storage_clone = Arc::clone(&self.storage);
        let provider_clone = Arc::clone(&self.provider);
        let config_clone = Arc::clone(&self.config);
        let hook_client = self.hook_client.clone();
        let tx_clone = tx.clone();

        let msg_loop_handle = tokio::spawn(async move {
            Self::run_message_processing_loop(
                storage_clone,
                provider_clone,
                config_clone,
                hook_client,
                tx_clone,
                shutdown_rx_msg,
            )
            .await
        });

        // Periodic sync loop
        let storage_clone = Arc::clone(&self.storage);
        let tx_clone = tx.clone();
        let sign_job_interval = self.config.signer.sign_job_interval;

        let sync_loop_handle = tokio::spawn(async move {
            Self::run_periodic_sync_loop(
                storage_clone,
                tx_clone,
                sign_job_interval,
                shutdown_rx_sync,
            )
            .await
        });

        // Wrap receiver in Arc<Mutex> for sharing among workers
        let rx = Arc::new(Mutex::new(rx));

        // Spawn N worker processing loops
        let mut worker_handles: Vec<JoinHandle<()>> = Vec::with_capacity(worker_count);
        for worker_id in 0..worker_count {
            let storage_clone = Arc::clone(&self.storage);
            let provider_clone = Arc::clone(&self.provider);
            let symbiotic_relay_client_clone = symbiotic_relay_client.clone();
            let shutdown_rx_worker = shutdown_rx.resubscribe();
            let rx_clone = Arc::clone(&rx);

            let handle = tokio::spawn(async move {
                Self::process_worker(
                    storage_clone,
                    provider_clone,
                    symbiotic_relay_client_clone,
                    key_tag,
                    worker_id,
                    rx_clone,
                    shutdown_rx_worker,
                )
                .await
            });
            worker_handles.push(handle);
        }

        // Combine all worker handles into a single future
        let workers_future = join_all(worker_handles);

        // Wait for shutdown or error
        tokio::select! {
            result = msg_loop_handle => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "message processing loop failed");
                }
            }
            result = sync_loop_handle => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "periodic sync loop failed");
                }
            }
            results = workers_future => {
                for (i, result) in results.into_iter().enumerate() {
                    if let Err(e) = result {
                        tracing::error!(error = %e, worker_id = i, "worker failed");
                    }
                }
            }
        }

        tracing::info!("signer job shutting down");
        Ok(())
    }

    /// Message processing loop
    async fn run_message_processing_loop(
        storage: Arc<Storage>,
        provider: DynProvider,
        config: Arc<AppConfig>,
        hook_client: reqwest::Client,
        tx: mpsc::Sender<MerkleRootWorkItem>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(config.signer.event_poll_interval);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("message processing loop shutting down");
                    return;
                }
                _ = interval.tick() => {
                    if let Err(e) = Self::process_messages_with_client(
                        &storage,
                        &provider,
                        &config,
                        &hook_client,
                        &tx,
                    ).await {
                        tracing::error!(error = %e, "error processing messages");
                    }
                }
            }
        }
    }

    /// Process pending messages and create merkle trees
    /// Messages arrive via webhook from OZ Monitor (already confirmed)
    async fn process_messages_with_client(
        storage: &Storage,
        provider: &DynProvider,
        config: &AppConfig,
        hook_client: &reqwest::Client,
        tx: &mpsc::Sender<MerkleRootWorkItem>,
    ) -> Result<(), SignerError> {
        // Load pending messages plus deferred messages whose hold has expired.
        let now = current_unix_timestamp();
        let messages = storage.list_messages_ready_for_acceptance(now)?;

        if messages.is_empty() {
            tracing::debug!("no messages ready for acceptance evaluation");
            return Ok(());
        }

        tracing::info!(
            count = messages.len(),
            "found messages ready for acceptance evaluation"
        );

        let mut accepted_messages: Vec<MessageData> = Vec::new();
        let mut deferred_count = 0usize;
        let mut rejected_count = 0usize;

        for msg in messages {
            match Self::evaluate_acceptance_hooks(storage, provider, config, hook_client, &msg)
                .await?
            {
                AcceptanceEvaluation::Accepted => accepted_messages.push(msg),
                AcceptanceEvaluation::Deferred => deferred_count += 1,
                AcceptanceEvaluation::Rejected => rejected_count += 1,
            }
        }

        tracing::info!(
            deferred = deferred_count,
            rejected = rejected_count,
            accepted = accepted_messages.len(),
            "filtered messages through acceptance hook"
        );

        // Group by (source_chain, destination_chain) pair
        let mut by_chain_pair: HashMap<(u64, u64), Vec<B256>> = HashMap::new();
        for msg in &accepted_messages {
            let src = msg.metadata.source_chain;
            let dest = msg.metadata.destination_chain;

            if !config.is_supported_destination(dest) {
                tracing::warn!(
                    message_id = %msg.metadata.message_id,
                    destination = dest,
                    "unsupported destination chain, skipping"
                );
                continue;
            }

            by_chain_pair
                .entry((src, dest))
                .or_default()
                .push(msg.metadata.message_id);
        }

        let provider_batch_size = provider.max_batch_size().max(1);

        // Build merkle tree batches for each (source, dest) pair
        for ((src_chain, dest_chain), msg_ids) in by_chain_pair {
            if msg_ids.is_empty() {
                continue;
            }

            if (msg_ids.len() as u64) < config.signer.min_batch_size {
                tracing::debug!(
                    src_chain,
                    dest_chain,
                    count = msg_ids.len(),
                    min_batch_size = config.signer.min_batch_size,
                    "waiting for more messages"
                );
                continue;
            }

            for batch in msg_ids.chunks(provider_batch_size) {
                let batch_ids = batch.to_vec();
                let tree = Self::build_merkle_tree(
                    storage,
                    provider,
                    batch_ids.clone(),
                    src_chain,
                    dest_chain,
                )?;
                let root = tree.root_hash;
                storage.save_merkle_tree(&tree)?;

                for msg_id in &batch_ids {
                    storage.update_message_status(msg_id, MessageStatus::Processing)?;
                }

                if tx
                    .send(MerkleRootWorkItem { root_hash: root })
                    .await
                    .is_err()
                {
                    tracing::error!("failed to enqueue merkle root for signing");
                }

                tracing::info!(
                    src_chain,
                    dest_chain,
                    root = %root,
                    leaves = tree.message_ids.len(),
                    batch_size = batch_ids.len(),
                    "created merkle tree"
                );
            }
        }

        Ok(())
    }

    #[cfg(test)]
    async fn process_messages(
        storage: &Storage,
        provider: &DynProvider,
        config: &AppConfig,
        tx: &mpsc::Sender<MerkleRootWorkItem>,
    ) -> Result<(), SignerError> {
        let hook_client = acceptance_hook_client();
        Self::process_messages_with_client(storage, provider, config, &hook_client, tx).await
    }

    async fn evaluate_acceptance_hooks(
        storage: &Storage,
        provider: &DynProvider,
        config: &AppConfig,
        hook_client: &reqwest::Client,
        msg: &MessageData,
    ) -> Result<AcceptanceEvaluation, SignerError> {
        let msg_id = msg.metadata.message_id;
        let mut state = storage.get_message_hook_state(&msg_id)?;
        let context = AcceptanceContext {
            defer_count: state.defer_count,
            previous_defer_reason: state.previous_defer_reason.clone(),
        };
        let mut selected_defer: Option<(DateTime<Utc>, Option<String>)> = None;

        if config.signer.acceptance_hooks.is_empty() {
            let decision = Self::evaluate_native_provider_hook(provider, msg, &context).await;
            if Self::apply_acceptance_decision(
                storage,
                &msg_id,
                &mut state,
                &mut selected_defer,
                decision,
            )? {
                return Ok(AcceptanceEvaluation::Rejected);
            }
        } else {
            for hook in &config.signer.acceptance_hooks {
                let decision = match hook {
                    AcceptanceHookConfig::Native { name } if name == "provider" => {
                        Self::evaluate_native_provider_hook(provider, msg, &context).await
                    }
                    AcceptanceHookConfig::Native { name } => AcceptanceDecision::reject(Some(
                        format!("unsupported native acceptance hook '{name}'"),
                    )),
                    AcceptanceHookConfig::Webhook {
                        url,
                        secret,
                        headers,
                        timeout,
                        error_backoff,
                        max_attempts,
                        ..
                    } => {
                        let key = hook.key();
                        match evaluate_webhook(
                            hook_client,
                            url,
                            secret,
                            headers,
                            *timeout,
                            msg,
                            &context,
                        )
                        .await
                        {
                            Ok(decision) => {
                                state.webhook_error_counts.remove(&key);
                                decision
                            }
                            Err(error) => {
                                let attempts = state
                                    .webhook_error_counts
                                    .entry(key.clone())
                                    .and_modify(|count| *count = count.saturating_add(1))
                                    .or_insert(1);
                                if *attempts >= *max_attempts {
                                    AcceptanceDecision::reject(Some(format!(
                                        "approval service unreachable after {} attempts",
                                        max_attempts
                                    )))
                                } else {
                                    tracing::warn!(
                                        message_id = %msg_id,
                                        hook = %key,
                                        attempts = *attempts,
                                        max_attempts,
                                        error = %error,
                                        "webhook acceptance hook failed; deferring message"
                                    );
                                    AcceptanceDecision::defer(
                                        Utc::now()
                                            + chrono::Duration::seconds(
                                                error_backoff.as_secs().min(i64::MAX as u64) as i64,
                                            ),
                                        Some(format!("acceptance hook '{}' error: {}", key, error)),
                                    )
                                }
                            }
                        }
                    }
                };

                if Self::apply_acceptance_decision(
                    storage,
                    &msg_id,
                    &mut state,
                    &mut selected_defer,
                    decision,
                )? {
                    return Ok(AcceptanceEvaluation::Rejected);
                }
            }
        }

        if let Some((until, reason)) = selected_defer {
            storage.mark_message_deferred(
                &msg_id,
                datetime_to_unix_seconds(until),
                reason.clone(),
                state,
            )?;
            tracing::info!(
                message_id = %msg_id,
                until = %until.to_rfc3339(),
                reason = reason.as_deref().unwrap_or(""),
                "message deferred by acceptance hooks"
            );
            return Ok(AcceptanceEvaluation::Deferred);
        }

        storage.clear_message_defer(&msg_id, state)?;
        Ok(AcceptanceEvaluation::Accepted)
    }

    async fn evaluate_native_provider_hook(
        provider: &DynProvider,
        msg: &MessageData,
        context: &AcceptanceContext,
    ) -> AcceptanceDecision {
        match provider.acceptance_hook(msg, context).await {
            Ok(decision) => decision,
            Err(error) => {
                AcceptanceDecision::reject(Some(format!("native hook 'provider' failed: {error}")))
            }
        }
    }

    fn apply_acceptance_decision(
        storage: &Storage,
        msg_id: &B256,
        state: &mut MessageHookState,
        selected_defer: &mut Option<(DateTime<Utc>, Option<String>)>,
        decision: AcceptanceDecision,
    ) -> Result<bool, SignerError> {
        match decision {
            AcceptanceDecision::Accept => Ok(false),
            AcceptanceDecision::Reject { reason } => {
                storage.mark_message_rejected(msg_id, reason.clone(), state.clone())?;
                tracing::warn!(
                    message_id = %msg_id,
                    reason = reason.as_deref().unwrap_or(""),
                    "message rejected by acceptance hook"
                );
                Ok(true)
            }
            AcceptanceDecision::Defer { until, reason } => {
                if selected_defer
                    .as_ref()
                    .is_none_or(|(current_until, _)| until >= *current_until)
                {
                    *selected_defer = Some((until, reason));
                }
                Ok(false)
            }
        }
    }

    /// Periodic sync loop - re-enqueue pending proofs
    async fn run_periodic_sync_loop(
        storage: Arc<Storage>,
        tx: mpsc::Sender<MerkleRootWorkItem>,
        interval: std::time::Duration,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(interval);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("periodic sync loop shutting down");
                    return;
                }
                _ = interval.tick() => {
                    if let Err(e) = Self::sync_pending_proofs(&storage, &tx).await {
                        tracing::error!(error = %e, "error during periodic sync");
                    }
                }
            }
        }
    }

    /// Sync pending proofs
    async fn sync_pending_proofs(
        storage: &Storage,
        tx: &mpsc::Sender<MerkleRootWorkItem>,
    ) -> Result<(), SignerError> {
        let pending_roots = storage.list_pending_merkle_roots()?;

        if pending_roots.is_empty() {
            tracing::debug!("no pending merkle roots to sync");
            return Ok(());
        }

        tracing::info!(
            count = pending_roots.len(),
            "periodic sync: found pending merkle roots"
        );

        let mut enqueued = 0;
        for root in pending_roots.keys() {
            if tx
                .send(MerkleRootWorkItem { root_hash: *root })
                .await
                .is_ok()
            {
                enqueued += 1;
            }
        }

        tracing::info!(enqueued, "periodic sync: enqueued pending merkle roots");
        Ok(())
    }

    /// Process worker
    async fn process_worker(
        storage: Arc<Storage>,
        provider: DynProvider,
        mut symbiotic_relay_client: SymbioticRelayClientEnum,
        key_tag: u32,
        worker_id: usize,
        rx: Arc<Mutex<mpsc::Receiver<MerkleRootWorkItem>>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        tracing::info!(worker_id, "signature worker started");
        loop {
            // Acquire lock briefly to receive work item
            let work_item = tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!(worker_id, "worker shutting down");
                    return;
                }
                item = async {
                    let mut rx_guard = rx.lock().await;
                    rx_guard.recv().await
                } => item,
            };

            let Some(work_item) = work_item else {
                tracing::info!(worker_id, "work channel closed, shutting down");
                return;
            };

            if let Err(e) = Self::process_single_root(
                &storage,
                &provider,
                &mut symbiotic_relay_client,
                key_tag,
                work_item.root_hash,
            )
            .await
            {
                tracing::error!(
                    worker_id,
                    error = %e,
                    root = %work_item.root_hash,
                    "error processing merkle root"
                );
            }
        }
    }

    /// Build merkle tree from message IDs using provider-specific leaf hashes.
    fn build_merkle_tree(
        storage: &Storage,
        provider: &DynProvider,
        msg_ids: Vec<B256>,
        source_chain: u64,
        dest_chain: u64,
    ) -> Result<MerkleTreeData, SignerError> {
        if msg_ids.is_empty() {
            return Err(SignerError::EmptyTree);
        }

        // Compute provider-specific leaf hashes for each message.
        let mut leaf_hashes = Vec::with_capacity(msg_ids.len());
        let mut valid_msg_ids = Vec::with_capacity(msg_ids.len());

        for msg_id in &msg_ids {
            let message = storage
                .get_message(msg_id)?
                .ok_or(SignerError::TreeNotFound)?;

            let leaf = provider
                .compute_leaf_hash(&message)
                .map_err(SignerError::Provider)?;

            leaf_hashes.push(leaf);
            valid_msg_ids.push(*msg_id);
        }

        // The tree only needs unique leaves, but every batched id must still
        // reach a terminal status — so `message_ids` keeps the full set even
        // when distinct messages hash to the same leaf (LayerZero replays,
        // re-ingestion under a fresh id).
        let mut sorted_leaves: Vec<B256> = leaf_hashes;
        sorted_leaves.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        let unique_leaf_count = {
            let mut u = sorted_leaves.clone();
            u.dedup();
            u.len()
        };
        sorted_leaves.dedup();

        let mut all_msg_ids = valid_msg_ids;
        if all_msg_ids.len() > unique_leaf_count {
            tracing::warn!(
                total = all_msg_ids.len(),
                unique_leaves = unique_leaf_count,
                source_chain,
                dest_chain,
                "batch contains messages with duplicate leaf hashes"
            );
        }
        all_msg_ids.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));

        let root = merkle_root(&sorted_leaves).ok_or(SignerError::EmptyTree)?;

        Ok(MerkleTreeData {
            root_hash: root,
            message_ids: all_msg_ids,
            leaf_hashes: sorted_leaves,
            source_chain,
            destination_chain: dest_chain,
            block_numbers: vec![], // No longer tracking block ranges
            proof: Vec::new(),
            epoch: None,       // Will be set when signature is requested
            attested_at: None, // Set when the aggregation proof arrives
        })
    }

    /// Process a single merkle root
    async fn process_single_root(
        storage: &Storage,
        provider: &DynProvider,
        symbiotic_relay_client: &mut SymbioticRelayClientEnum,
        key_tag: u32,
        root_hash: B256,
    ) -> Result<(), SignerError> {
        // Check if we already have a request ID for this root
        let request_id = storage.get_pending_request_id(&root_hash)?;

        let request_id = match request_id {
            Some(id) => id,
            None => {
                // Get the merkle tree to find destination chain
                let tree = storage
                    .get_merkle_tree_by_root(&root_hash)?
                    .ok_or(SignerError::TreeNotFound)?;

                // Encode provider-specific signing message (sidecar hashes internally).
                let signing_message = provider
                    .encode_signing_message(&tree)
                    .map_err(SignerError::Provider)?;
                let expected_hash = alloy::primitives::keccak256(&signing_message);

                let resp = symbiotic_relay_client
                    .sign_message(&signing_message, key_tag)
                    .await?;

                storage.set_pending_request_id(&root_hash, &resp.request_id)?;

                if let Ok(Some(mut tree)) = storage.get_merkle_tree_by_root(&root_hash) {
                    tree.epoch = Some(resp.epoch);
                    let _ = storage.save_merkle_tree(&tree);
                }

                tracing::info!(
                    root = %root_hash,
                    expected_hash = %expected_hash,
                    dest_chain = tree.destination_chain,
                    provider = provider.name(),
                    request_id = %resp.request_id,
                    epoch = resp.epoch,
                    "submitted for signing"
                );
                return Ok(());
            }
        };

        // Try to get aggregation proof
        match symbiotic_relay_client
            .get_aggregation_proof(&request_id)
            .await
        {
            Ok(resp) => {
                // Success - attach proof to merkle tree
                let mut tree = storage
                    .get_merkle_tree_by_root(&root_hash)?
                    .ok_or(SignerError::TreeNotFound)?;

                if let Some(agg_proof) = resp.aggregation_proof {
                    tree.proof = agg_proof.proof;
                    tree.attested_at = Some(crate::storage::unix_timestamp());
                }
                storage.save_merkle_tree(&tree)?;

                // Update message statuses to Signed
                for msg_id in &tree.message_ids {
                    if *msg_id != B256::ZERO {
                        storage.update_message_status(msg_id, MessageStatus::Signed)?;
                    }
                }

                // Remove from pending
                storage.delete_pending(&root_hash)?;

                tracing::info!(root = %root_hash, "proof attached");
                Ok(())
            }
            Err(crate::error::SymbioticRelayError::NotReady) => {
                // Proof not ready yet - will retry on next sync cycle
                tracing::debug!(root = %root_hash, "proof not ready");
                Err(SignerError::ProofNotReady)
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::evm::DecodedJobAssigned;
    use crate::provider::Provider;
    use crate::storage::{MerkleTreeData, MessageData, MessageMetadata};
    use crate::symbiotic_relay::MockSymbioticRelayClient;
    use alloy::primitives::{Address, B256};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::time::Duration;
    use tempfile::tempdir;
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_storage() -> (Arc<Storage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        (Arc::new(storage), dir)
    }

    fn test_config() -> Arc<AppConfig> {
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
                acceptance_hooks: Vec::new(),
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338, 42161],
            provider: "layerzero".to_string(),
            layerzero: Some(LayerZeroConfig {
                eid_to_chain_id: {
                    let mut map = HashMap::new();
                    map.insert(40231, 31337);
                    map.insert(40232, 31338);
                    map
                },
                target_addresses: {
                    let mut map = HashMap::new();
                    map.insert(
                        31338,
                        "0x1234567890123456789012345678901234567890".to_string(),
                    );
                    map.insert(
                        42161,
                        "0xabcdef0123456789abcdef0123456789abcdef01".to_string(),
                    );
                    map
                },
            }),
            chainlink_ccv: None,
        })
    }

    fn test_job_assigned(guid: B256) -> DecodedJobAssigned {
        DecodedJobAssigned {
            guid,
            src_eid: 40231,
            dst_eid: 40232,
            sender: Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x03u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 1,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        }
    }

    fn test_message(id: B256) -> MessageData {
        let job = test_job_assigned(id);
        MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 12345,
                message_id: id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        }
    }

    struct RejectAllProvider;

    #[async_trait]
    impl Provider for RejectAllProvider {
        fn name(&self) -> &'static str {
            "reject"
        }

        async fn handle_webhook_event(
            &self,
            _event: &crate::webhook::WebhookEvent,
        ) -> Result<(), crate::error::ProviderError> {
            Ok(())
        }

        async fn acceptance_hook(
            &self,
            _msg: &MessageData,
            _context: &crate::acceptance::AcceptanceContext,
        ) -> Result<crate::acceptance::AcceptanceDecision, crate::error::ProviderError> {
            Ok(crate::acceptance::AcceptanceDecision::Reject {
                reason: Some("rejected".to_string()),
            })
        }

        fn compute_leaf_hash(
            &self,
            message: &MessageData,
        ) -> Result<B256, crate::error::ProviderError> {
            Ok(alloy::primitives::keccak256(
                message.metadata.message_id.as_slice(),
            ))
        }

        fn encode_signing_message(
            &self,
            tree: &MerkleTreeData,
        ) -> Result<Vec<u8>, crate::error::ProviderError> {
            Ok(tree.root_hash.as_slice().to_vec())
        }
    }

    struct MaxBatch2Provider;

    struct DeferAllProvider;

    #[async_trait]
    impl Provider for MaxBatch2Provider {
        fn name(&self) -> &'static str {
            "maxbatch2"
        }

        async fn handle_webhook_event(
            &self,
            _event: &crate::webhook::WebhookEvent,
        ) -> Result<(), crate::error::ProviderError> {
            Ok(())
        }

        fn max_batch_size(&self) -> usize {
            2
        }

        fn compute_leaf_hash(
            &self,
            message: &MessageData,
        ) -> Result<B256, crate::error::ProviderError> {
            Ok(alloy::primitives::keccak256(
                message.metadata.message_id.as_slice(),
            ))
        }

        fn encode_signing_message(
            &self,
            tree: &MerkleTreeData,
        ) -> Result<Vec<u8>, crate::error::ProviderError> {
            Ok(tree.root_hash.as_slice().to_vec())
        }
    }

    #[async_trait]
    impl Provider for DeferAllProvider {
        fn name(&self) -> &'static str {
            "defer"
        }

        async fn handle_webhook_event(
            &self,
            _event: &crate::webhook::WebhookEvent,
        ) -> Result<(), crate::error::ProviderError> {
            Ok(())
        }

        async fn acceptance_hook(
            &self,
            _msg: &MessageData,
            _context: &crate::acceptance::AcceptanceContext,
        ) -> Result<crate::acceptance::AcceptanceDecision, crate::error::ProviderError> {
            Ok(crate::acceptance::AcceptanceDecision::Defer {
                until: Utc::now() + chrono::Duration::seconds(60),
                reason: Some("awaiting approval".to_string()),
            })
        }

        fn compute_leaf_hash(
            &self,
            message: &MessageData,
        ) -> Result<B256, crate::error::ProviderError> {
            Ok(alloy::primitives::keccak256(
                message.metadata.message_id.as_slice(),
            ))
        }

        fn encode_signing_message(
            &self,
            tree: &MerkleTreeData,
        ) -> Result<Vec<u8>, crate::error::ProviderError> {
            Ok(tree.root_hash.as_slice().to_vec())
        }
    }

    struct AllowAllProvider;

    #[async_trait]
    impl Provider for AllowAllProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn handle_webhook_event(
            &self,
            _event: &crate::webhook::WebhookEvent,
        ) -> Result<(), crate::error::ProviderError> {
            Ok(())
        }

        fn compute_leaf_hash(
            &self,
            message: &MessageData,
        ) -> Result<B256, crate::error::ProviderError> {
            Ok(alloy::primitives::keccak256(
                message.metadata.message_id.as_slice(),
            ))
        }

        fn encode_signing_message(
            &self,
            tree: &MerkleTreeData,
        ) -> Result<Vec<u8>, crate::error::ProviderError> {
            Ok(tree.root_hash.as_slice().to_vec())
        }
    }

    fn test_layerzero_provider(config: Arc<AppConfig>, storage: Arc<Storage>) -> DynProvider {
        Arc::new(crate::provider::LayerZeroProvider::new(
            config.layerzero.clone().unwrap_or_default(),
            config,
            storage,
        ))
    }

    #[test]
    fn test_signer_job_new() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider =
            test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        let job = SignerJob::new(Arc::clone(&storage), provider, config);
        // Just verify it compiles and creates without panic
        drop(job);
    }

    #[test]
    fn test_build_merkle_tree_single_message() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let msg_id = B256::from_slice(&[0x01u8; 32]);

        // Save message with job_assigned data
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let tree =
            SignerJob::build_merkle_tree(&storage, &provider, vec![msg_id], 31337, 31338).unwrap();

        assert_eq!(
            tree.leaf_hashes.len(),
            1,
            "single message should produce single leaf"
        );
        assert_eq!(tree.message_ids.len(), 1, "should have 1 message_id");
        assert!(tree.proof.is_empty(), "new tree should have no proof");
        assert!(tree.epoch.is_none(), "new tree should have no epoch");
    }

    #[test]
    fn test_build_merkle_tree_multiple_messages() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let msg_ids: Vec<B256> = (1..=3).map(|i| B256::from_slice(&[i; 32])).collect();

        // Save messages
        for msg_id in &msg_ids {
            let msg = test_message(*msg_id);
            storage.save_message(&msg).unwrap();
        }

        let tree = SignerJob::build_merkle_tree(&storage, &provider, msg_ids.clone(), 31337, 31338)
            .unwrap();

        assert_eq!(tree.source_chain, 31337);
        assert_eq!(tree.destination_chain, 31338);
        // Messages may be deduplicated if they have same leaf hash
        assert!(!tree.root_hash.is_zero(), "should have valid root hash");
    }

    #[test]
    fn test_build_merkle_tree_preserves_chain_info() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let tree =
            SignerJob::build_merkle_tree(&storage, &provider, vec![msg_id], 1, 42161).unwrap();

        assert_eq!(tree.source_chain, 1);
        assert_eq!(tree.destination_chain, 42161);
    }

    #[test]
    fn test_build_merkle_tree_message_not_found() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let msg_id = B256::from_slice(&[0x99u8; 32]); // Does not exist

        let result = SignerJob::build_merkle_tree(&storage, &provider, vec![msg_id], 31337, 31338);

        assert!(result.is_err());
    }

    #[test]
    fn test_encode_signing_message_success() {
        let config = test_config();
        let (storage, _dir) = test_storage();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338, // Has target address configured
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };

        let result = provider.encode_signing_message(&tree);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        // ABI encoded: uint256 chainId (32 bytes) + address (32 bytes) + bytes32 root (32 bytes)
        assert_eq!(encoded.len(), 96);
    }

    #[test]
    fn test_encode_signing_message_missing_target() {
        let config = test_config();
        let (storage, _dir) = test_storage();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 99999, // No target address configured
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };

        let result = provider.encode_signing_message(&tree);
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("target address not configured"));
    }

    #[test]
    fn test_merkle_root_work_item() {
        let root = B256::from_slice(&[0xAAu8; 32]);
        let work_item = MerkleRootWorkItem { root_hash: root };
        assert_eq!(work_item.root_hash, root);

        // Test clone
        let cloned = work_item.clone();
        assert_eq!(cloned.root_hash, root);
    }

    #[test]
    fn test_message_status_transitions() {
        let (storage, _dir) = test_storage();
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);

        // Save message (starts as Pending)
        storage.save_message(&msg).unwrap();

        // List pending
        let pending = storage
            .list_messages_by_status(MessageStatus::Pending)
            .unwrap();
        assert_eq!(pending.len(), 1);

        // Update to Processing
        storage
            .update_message_status(&msg_id, MessageStatus::Processing)
            .unwrap();
        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(processing.len(), 1);

        // Update to Signed
        storage
            .update_message_status(&msg_id, MessageStatus::Signed)
            .unwrap();
        let signed = storage
            .list_messages_by_status(MessageStatus::Signed)
            .unwrap();
        assert_eq!(signed.len(), 1);
    }

    #[test]
    fn test_merkle_tree_sorted_leaves() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        // Create two messages
        let msg1_id = B256::from_slice(&[0x01u8; 32]);
        let msg2_id = B256::from_slice(&[0x02u8; 32]);

        let msg1 = test_message(msg1_id);
        let msg2 = test_message(msg2_id);
        storage.save_message(&msg1).unwrap();
        storage.save_message(&msg2).unwrap();

        let tree =
            SignerJob::build_merkle_tree(&storage, &provider, vec![msg1_id, msg2_id], 31337, 31338)
                .unwrap();

        // Verify leaves are sorted
        for i in 1..tree.leaf_hashes.len() {
            assert!(
                tree.leaf_hashes[i - 1].as_slice() <= tree.leaf_hashes[i].as_slice(),
                "leaves should be sorted"
            );
        }
    }

    #[test]
    fn test_pending_request_id_workflow() {
        let (storage, _dir) = test_storage();
        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create a tree (will be added to pending)
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Should be pending without request ID
        let req_id = storage.get_pending_request_id(&root).unwrap();
        assert!(req_id.is_none());

        // Set request ID
        storage
            .set_pending_request_id(&root, "mock-request-123")
            .unwrap();

        // Should have request ID now
        let req_id = storage.get_pending_request_id(&root).unwrap();
        assert_eq!(req_id, Some("mock-request-123".to_string()));
    }

    #[test]
    fn test_signed_tree_not_pending() {
        let (storage, _dir) = test_storage();
        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create a signed tree (has proof)
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![0u8; 96], // Non-empty = signed
            epoch: Some(1),
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Should not be in pending list
        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(!pending.contains_key(&root));
    }

    #[test]
    fn test_encode_signing_message_no_layerzero_config() {
        let config = Arc::new(AppConfig {
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
                acceptance_hooks: Vec::new(),
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338, 42161],
            chainlink_ccv: None,
            provider: "layerzero".to_string(),
            layerzero: None, // No LayerZero config
        });
        let (storage, _dir) = test_storage();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };

        let result = provider.encode_signing_message(&tree);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_merkle_tree_deduplicates_same_leaf() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        // Create two messages that will have the same leaf hash
        // (same packet_header, payload_hash, confirmations)
        let msg1_id = B256::from_slice(&[0x01u8; 32]);
        let msg2_id = B256::from_slice(&[0x02u8; 32]);

        // Save both with same job data
        let job = test_job_assigned(msg1_id);
        let msg1 = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 12345,
                message_id: msg1_id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg1).unwrap();

        // Create second message with same job data (same leaf hash)
        let msg2 = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 12346,
                message_id: msg2_id,
                event_tx_hash: B256::from_slice(&[0x03u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg2).unwrap();

        let tree =
            SignerJob::build_merkle_tree(&storage, &provider, vec![msg1_id, msg2_id], 31337, 31338)
                .unwrap();

        // Due to deduplication, should have fewer unique leaves
        assert!(tree.leaf_hashes.len() <= tree.message_ids.len());
    }

    #[test]
    fn test_encode_signing_message_content() {
        let config = test_config();
        let (storage, _dir) = test_storage();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338, // Has target address configured
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };

        let result = provider.encode_signing_message(&tree).unwrap();

        // Should contain chain ID (31338 = 0x7A6A in hex, padded to 32 bytes)
        // Position 24-32 should contain the chain ID as u64
        let chain_id_bytes = &result[24..32];
        let chain_id = u64::from_be_bytes(chain_id_bytes.try_into().unwrap());
        assert_eq!(chain_id, 31338);
    }

    #[test]
    fn test_build_merkle_tree_empty_input() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        // Building a tree with no messages should fail
        let result = SignerJob::build_merkle_tree(&storage, &provider, vec![], 31337, 31338);

        // Empty tree returns error (EmptyTree)
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_messages_respects_min_batch_size() {
        let (storage, _dir) = test_storage();
        let mut config = (*test_config()).clone();
        config.signer.min_batch_size = 5; // Need at least 5 messages
        let config = Arc::new(config);
        let provider: DynProvider = Arc::new(AllowAllProvider);

        // Save only 1 message (below min_batch_size)
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should not have enqueued anything since we only have 1 message
        assert!(rx.try_recv().is_err());

        // Message should still be pending
        let pending = storage
            .list_messages_by_status(MessageStatus::Pending)
            .unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_process_messages_skips_unsupported_destination() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(AllowAllProvider);

        // Save a message with unsupported destination
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let job = test_job_assigned(msg_id);
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 99999, // Not in destination_chains
                block_number: 12345,
                message_id: msg_id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should not enqueue anything (unsupported destination)
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_process_messages_no_pending() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(AllowAllProvider);

        let (tx, mut rx) = mpsc::channel(10);

        // No messages at all
        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_sync_pending_proofs_enqueues() {
        let (storage, _dir) = test_storage();

        // Create a pending merkle tree (unsigned, so it appears in pending list)
        let root = B256::from_slice(&[0xAAu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![], // empty = unsigned = pending
            epoch: None,
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::sync_pending_proofs(&storage, &tx).await.unwrap();

        let item = rx.try_recv().unwrap();
        assert_eq!(item.root_hash, root);
    }

    #[tokio::test]
    async fn test_sync_pending_proofs_empty() {
        let (storage, _dir) = test_storage();
        let (tx, mut rx) = mpsc::channel(10);

        // No pending roots
        SignerJob::sync_pending_proofs(&storage, &tx).await.unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_process_messages_rejects_via_acceptance_hook() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(RejectAllProvider);

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should not enqueue anything (all rejected)
        assert!(rx.try_recv().is_err());

        let rejected = storage
            .list_messages_by_status(MessageStatus::Rejected)
            .unwrap();
        assert_eq!(rejected.len(), 1);

        let state = storage.get_message_hook_state(&msg_id).unwrap();
        assert_eq!(state.rejected_reason.as_deref(), Some("rejected"));
    }

    #[tokio::test]
    async fn test_process_messages_defers_via_acceptance_hook() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(DeferAllProvider);

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());

        let deferred = storage
            .list_messages_by_status(MessageStatus::Deferred)
            .unwrap();
        assert_eq!(deferred.len(), 1);

        let state = storage.get_message_hook_state(&msg_id).unwrap();
        assert_eq!(state.defer_count, 1);
        assert_eq!(
            state.previous_defer_reason.as_deref(),
            Some("awaiting approval")
        );
    }

    #[tokio::test]
    async fn test_process_messages_webhook_errors_reject_after_max_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let (storage, _dir) = test_storage();
        let mut config = (*test_config()).clone();
        config.signer.acceptance_hooks = vec![crate::acceptance::AcceptanceHookConfig::Webhook {
            name: Some("approval".to_string()),
            url: server.uri(),
            secret: "shared-secret".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(5),
            error_backoff: Duration::from_secs(1),
            max_attempts: 2,
        }];
        let config = Arc::new(config);
        let provider: DynProvider = Arc::new(AllowAllProvider);

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
        let mut state = storage.get_message_hook_state(&msg_id).unwrap();
        assert_eq!(state.defer_count, 1);
        assert_eq!(state.webhook_error_counts.get("webhook:approval"), Some(&1));

        state.deferred_until = Some(0);
        storage.save_message_hook_state(&msg_id, &state).unwrap();

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        let rejected = storage
            .list_messages_by_status(MessageStatus::Rejected)
            .unwrap();
        assert_eq!(rejected.len(), 1);

        let state = storage.get_message_hook_state(&msg_id).unwrap();
        assert_eq!(
            state.rejected_reason.as_deref(),
            Some("approval service unreachable after 2 attempts")
        );
    }

    #[tokio::test]
    async fn test_acceptance_hook_client_does_not_follow_redirects() {
        let redirect_target = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "accept"
            })))
            .expect(0)
            .mount(&redirect_target)
            .await;

        let redirect_source = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", redirect_target.uri()),
            )
            .expect(1)
            .mount(&redirect_source)
            .await;

        let err = evaluate_webhook(
            &acceptance_hook_client(),
            &redirect_source.uri(),
            "shared-secret",
            &HashMap::new(),
            Duration::from_secs(5),
            &test_message(B256::from_slice(&[0x01u8; 32])),
            &AcceptanceContext {
                defer_count: 0,
                previous_defer_reason: None,
            },
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("302"));
        redirect_source.verify().await;
        redirect_target.verify().await;
    }

    #[tokio::test]
    async fn test_process_messages_batches_by_max_batch_size() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(MaxBatch2Provider);

        // Create 4 messages on same (src, dst)
        for i in 1..=4u8 {
            let msg_id = B256::from_slice(&[i; 32]);
            let msg = test_message(msg_id);
            storage.save_message(&msg).unwrap();
        }

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should have created 2 batches (4 messages / max_batch_size=2)
        let item1 = rx.try_recv().unwrap();
        let item2 = rx.try_recv().unwrap();

        let tree1 = storage
            .get_merkle_tree_by_root(&item1.root_hash)
            .unwrap()
            .unwrap();
        let tree2 = storage
            .get_merkle_tree_by_root(&item2.root_hash)
            .unwrap()
            .unwrap();

        assert_eq!(tree1.message_ids.len(), 2);
        assert_eq!(tree2.message_ids.len(), 2);
        assert_ne!(item1.root_hash, item2.root_hash);
    }

    #[tokio::test]
    async fn test_process_messages_groups_by_chain_pair() {
        let (storage, _dir) = test_storage();
        let config = test_config(); // destination_chains = [31338, 42161]
        let provider: DynProvider = Arc::new(AllowAllProvider);

        // Message to chain 31338
        let msg1_id = B256::from_slice(&[0x01u8; 32]);
        let job1 = test_job_assigned(msg1_id);
        let msg1 = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id: msg1_id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job1).unwrap(),
        };
        storage.save_message(&msg1).unwrap();

        // Message to chain 42161
        let msg2_id = B256::from_slice(&[0x02u8; 32]);
        let job2 = test_job_assigned(msg2_id);
        let msg2 = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 42161,
                block_number: 101,
                message_id: msg2_id,
                event_tx_hash: B256::from_slice(&[0x03u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job2).unwrap(),
        };
        storage.save_message(&msg2).unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should have enqueued 2 items (one per chain pair)
        let item1 = rx.try_recv();
        let item2 = rx.try_recv();
        assert!(item1.is_ok());
        assert!(item2.is_ok());

        // All messages should be Processing now
        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(processing.len(), 2);
    }

    #[tokio::test]
    async fn test_process_messages_enqueues_and_updates_status() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(AllowAllProvider);

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let (tx, mut rx) = mpsc::channel(1);

        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        let item = rx.recv().await.unwrap();
        assert_eq!(
            item.root_hash,
            storage
                .list_pending_merkle_roots()
                .unwrap()
                .keys()
                .next()
                .copied()
                .unwrap()
        );

        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(processing.len(), 1);
        assert_eq!(processing[0].metadata.message_id, msg_id);
    }

    #[tokio::test]
    async fn test_process_single_root_tree_not_found() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        // Root that doesn't exist in storage
        let root = B256::from_slice(&[0xBBu8; 32]);

        let result =
            SignerJob::process_single_root(&storage, &provider, &mut client, 15, root).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::TreeNotFound => {}
            other => panic!("expected TreeNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_process_single_root_already_has_request_id_but_proof_not_ready() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));

        let msg_id = B256::from_slice(&[0x10u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Set a pending request ID directly
        storage
            .set_pending_request_id(&root, "mock-request-999")
            .unwrap();

        // Mock client returns proof immediately, so the second call succeeds
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());
        let result =
            SignerJob::process_single_root(&storage, &provider, &mut client, 15, root).await;
        assert!(result.is_ok());

        // Verify proof was attached
        let tree = storage.get_merkle_tree_by_root(&root).unwrap().unwrap();
        assert!(!tree.proof.is_empty());
    }

    #[tokio::test]
    async fn test_sync_pending_proofs_with_closed_channel() {
        let (storage, _dir) = test_storage();

        // Create a pending merkle tree
        let root = B256::from_slice(&[0xCCu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let (tx, _rx) = mpsc::channel(10);
        // Drop receiver to close channel - sends will fail silently
        drop(_rx);

        // Should not panic even though channel is closed
        let result = SignerJob::sync_pending_proofs(&storage, &tx).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_merkle_tree_with_generic_provider() {
        let (storage, _dir) = test_storage();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let job = test_job_assigned(msg_id);
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 12345,
                message_id: msg_id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let provider: DynProvider = Arc::new(AllowAllProvider);
        let tree =
            SignerJob::build_merkle_tree(&storage, &provider, vec![msg_id], 31337, 31338).unwrap();

        assert_eq!(tree.source_chain, 31337);
        assert_eq!(tree.destination_chain, 31338);
        assert!(!tree.root_hash.is_zero());
    }

    #[tokio::test]
    async fn test_process_messages_creates_tree_and_enqueues() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider: DynProvider = Arc::new(AllowAllProvider);

        // Create 2 messages for same chain pair
        for i in 1..=2u8 {
            let msg_id = B256::from_slice(&[i; 32]);
            let msg = test_message(msg_id);
            storage.save_message(&msg).unwrap();
        }

        let (tx, mut rx) = mpsc::channel(10);
        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        // Should have enqueued at least one work item
        let item = rx.try_recv();
        assert!(item.is_ok());
    }

    #[tokio::test]
    async fn test_process_single_root_sets_request_id_then_attaches_proof() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        let msg_id = B256::from_slice(&[0x10u8; 32]);
        let msg = test_message(msg_id);
        storage.save_message(&msg).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
            attested_at: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // First call should set pending request ID and epoch
        SignerJob::process_single_root(&storage, &provider, &mut client, 15, root)
            .await
            .unwrap();

        let req_id = storage.get_pending_request_id(&root).unwrap();
        assert!(req_id.is_some());

        let tree = storage.get_merkle_tree_by_root(&root).unwrap().unwrap();
        assert!(tree.epoch.is_some());

        // Second call should attach proof and mark message Signed
        SignerJob::process_single_root(&storage, &provider, &mut client, 15, root)
            .await
            .unwrap();

        let tree = storage.get_merkle_tree_by_root(&root).unwrap().unwrap();
        assert!(!tree.proof.is_empty());

        let signed = storage
            .list_messages_by_status(MessageStatus::Signed)
            .unwrap();
        assert_eq!(signed.len(), 1);
        assert_eq!(signed[0].metadata.message_id, msg_id);

        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(!pending.contains_key(&root));
    }

    /// Duplicate-leaf batches must not leave messages stuck in Processing
    /// after the surviving leaf is signed.
    #[tokio::test]
    async fn test_duplicate_leaf_batch_all_messages_reach_signed() {
        let (storage, _dir) = test_storage();
        let config = test_config();
        let provider = test_layerzero_provider(Arc::clone(&config), Arc::clone(&storage));
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        // Two distinct message ids carrying identical LayerZero payloads
        // (same packet_header / payload_hash / confirmations) → identical leaf.
        let msg1_id = B256::from_slice(&[0x01u8; 32]);
        let msg2_id = B256::from_slice(&[0x02u8; 32]);
        let job = test_job_assigned(B256::ZERO);
        let make_msg = |id: B256, block: u64, tx: u8| MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: block,
                message_id: id,
                event_tx_hash: B256::from_slice(&[tx; 32]),
                ttl: None,
            },
            data: serde_json::to_vec(&job).unwrap(),
        };
        storage.save_message(&make_msg(msg1_id, 1, 0xAA)).unwrap();
        storage.save_message(&make_msg(msg2_id, 2, 0xBB)).unwrap();

        let (tx, _rx) = mpsc::channel(10);
        SignerJob::process_messages(&storage, &provider, &config, &tx)
            .await
            .unwrap();

        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(
            processing.len(),
            2,
            "both messages should be Processing after batching"
        );

        let pending = storage.list_pending_merkle_roots().unwrap();
        assert_eq!(pending.len(), 1, "duplicate-leaf batch produces one root");
        let root = *pending.keys().next().unwrap();

        // First call submits signing request, second call attaches the proof
        // and triggers the Processing → Signed transition.
        SignerJob::process_single_root(&storage, &provider, &mut client, 15, root)
            .await
            .unwrap();
        SignerJob::process_single_root(&storage, &provider, &mut client, 15, root)
            .await
            .unwrap();

        let stuck = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert!(
            stuck.is_empty(),
            "no messages should be stuck in Processing, found {}",
            stuck.len()
        );
        let signed = storage
            .list_messages_by_status(MessageStatus::Signed)
            .unwrap();
        assert_eq!(signed.len(), 2, "both messages must be Signed");
    }
}
