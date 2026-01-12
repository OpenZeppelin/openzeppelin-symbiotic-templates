use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::B256;
use futures::future::join_all;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::config::AppConfig;
use crate::crypto::{compute_dvn_leaf, merkle_root};
use crate::error::SignerError;
use crate::evm::DecodedJobAssigned;
use crate::provider::DynProvider;
use crate::symbiotic_relay::SymbioticRelayClientEnum;
use crate::storage::{MerkleTreeData, MessageStatus, Storage};

/// Merkle root work item
#[derive(Debug, Clone)]
pub struct MerkleRootWorkItem {
    pub root_hash: B256,
}

/// Signer job that manages merkle tree creation and signing
pub struct SignerJob {
    storage: Arc<Storage>,
    provider: DynProvider,
    config: Arc<AppConfig>,
}

impl SignerJob {
    /// Create a new signer job
    pub fn new(
        storage: Arc<Storage>,
        provider: DynProvider,
        config: Arc<AppConfig>,
    ) -> Self {
        Self {
            storage,
            provider,
            config,
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
        let tx_clone = tx.clone();

        let msg_loop_handle = tokio::spawn(async move {
            Self::run_message_processing_loop(
                storage_clone,
                provider_clone,
                config_clone,
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
            let symbiotic_relay_client_clone = symbiotic_relay_client.clone();
            let shutdown_rx_worker = shutdown_rx.resubscribe();
            let rx_clone = Arc::clone(&rx);

            let handle = tokio::spawn(async move {
                Self::process_worker(
                    storage_clone,
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
                    if let Err(e) = Self::process_messages(
                        &storage,
                        &provider,
                        &config,
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
    async fn process_messages(
        storage: &Storage,
        provider: &DynProvider,
        config: &AppConfig,
        tx: &mpsc::Sender<MerkleRootWorkItem>,
    ) -> Result<(), SignerError> {
        // Load all pending messages (confirmed by OZ Monitor, awaiting processing)
        let messages = storage.list_messages_by_status(MessageStatus::Pending)?;

        if messages.is_empty() {
            tracing::debug!("no pending messages to process");
            return Ok(());
        }

        tracing::info!(count = messages.len(), "found pending messages to process");

        // Filter through acceptance hook
        let mut accepted_ids: Vec<B256> = Vec::new();
        for msg in &messages {
            match provider.acceptance_hook(msg).await {
                Ok(()) => accepted_ids.push(msg.metadata.message_id),
                Err(e) => {
                    tracing::warn!(
                        message_id = %msg.metadata.message_id,
                        error = %e,
                        "message rejected by acceptance hook"
                    );
                }
            }
        }

        // Filter messages to only include those accepted by the hook
        let accepted_messages: Vec<_> = messages
            .iter()
            .filter(|msg| accepted_ids.contains(&msg.metadata.message_id))
            .collect();

        tracing::info!(
            total = messages.len(),
            accepted = accepted_messages.len(),
            "filtered messages through acceptance hook"
        );

        // Group by (source_chain, destination_chain) pair
        let mut by_chain_pair: HashMap<(u64, u64), Vec<B256>> = HashMap::new();
        for msg in &accepted_messages {
            let src = msg.metadata.source_chain;
            let dest = msg.metadata.destination_chain;

            if !Self::is_supported_destination(config, dest) {
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

        // Build merkle tree for each (source, dest) pair
        for ((src_chain, dest_chain), msg_ids) in by_chain_pair {
            if msg_ids.is_empty() {
                continue;
            }

            // Check minimum batch size
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

            let tree = Self::build_merkle_tree(storage, msg_ids.clone(), src_chain, dest_chain)?;
            let root = tree.root_hash;
            storage.save_merkle_tree(&tree)?;

            // Mark messages as processing
            for msg_id in &msg_ids {
                storage.update_message_status(msg_id, MessageStatus::Processing)?;
            }

            // Enqueue for signing
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
                "created merkle tree"
            );
        }

        Ok(())
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
                &mut symbiotic_relay_client,
                key_tag,
                work_item.root_hash,
            ).await {
                tracing::error!(
                    worker_id,
                    error = %e,
                    root = %work_item.root_hash,
                    "error processing merkle root"
                );
            }
        }
    }

    /// Check if destination chain is supported
    fn is_supported_destination(config: &AppConfig, dest: u64) -> bool {
        config.is_supported_destination(dest)
    }

    /// Build merkle tree from message IDs with DVN-compatible leaf hashes
    fn build_merkle_tree(
        storage: &Storage,
        msg_ids: Vec<B256>,
        source_chain: u64,
        dest_chain: u64,
    ) -> Result<MerkleTreeData, SignerError> {
        // Compute DVN-compatible leaf hashes for each message
        let mut leaf_hashes = Vec::with_capacity(msg_ids.len());
        let mut valid_msg_ids = Vec::with_capacity(msg_ids.len());

        for msg_id in &msg_ids {
            // Load message from storage
            let message = storage
                .get_message(msg_id)?
                .ok_or(SignerError::TreeNotFound)?;

            // Deserialize DecodedJobAssigned from message data
            let job_assigned: DecodedJobAssigned = serde_json::from_slice(&message.data)
                .map_err(|e| SignerError::EvmClient(format!("failed to deserialize job: {}", e)))?;

            // Compute DVN-compatible leaf hash
            let leaf = compute_dvn_leaf(
                &job_assigned.packet_header,
                job_assigned.payload_hash,
                job_assigned.confirmations,
            );

            leaf_hashes.push(leaf);
            valid_msg_ids.push(*msg_id);
        }

        // Handle single-message case by adding zero hash
        if leaf_hashes.len() == 1 {
            leaf_hashes.push(B256::ZERO);
            valid_msg_ids.push(B256::ZERO);
        }

        // Sort and dedup leaves (keeping message_ids in sync)
        // Note: We sort by leaf_hash to match DVN contract expectations
        let mut indexed: Vec<(B256, B256)> = leaf_hashes
            .into_iter()
            .zip(valid_msg_ids)
            .collect();
        indexed.sort_by(|a, b| a.0.as_slice().cmp(b.0.as_slice()));
        indexed.dedup_by(|a, b| a.0 == b.0);

        let sorted_leaves: Vec<B256> = indexed.iter().map(|(leaf, _)| *leaf).collect();
        let sorted_msg_ids: Vec<B256> = indexed.iter().map(|(_, msg_id)| *msg_id).collect();

        let root = merkle_root(&sorted_leaves).ok_or(SignerError::EmptyTree)?;

        Ok(MerkleTreeData {
            root_hash: root,
            message_ids: sorted_msg_ids,
            leaf_hashes: sorted_leaves,
            source_chain,
            destination_chain: dest_chain,
            block_numbers: vec![], // No longer tracking block ranges
            proof: Vec::new(),
            epoch: None, // Will be set when signature is requested
        })
    }

    /// Process a single merkle root
    async fn process_single_root(
        storage: &Storage,
        symbiotic_relay_client: &mut SymbioticRelayClientEnum,
        key_tag: u32,
        root_hash: B256,
    ) -> Result<(), SignerError> {
        // Check if we already have a request ID for this root
        let request_id = storage.get_pending_request_id(&root_hash)?;

        if request_id.is_none() {
            // No request ID yet - submit for signing
            let resp = symbiotic_relay_client
                .sign_message(root_hash.as_slice(), key_tag)
                .await?;

            // Store request ID for tracking
            storage.set_pending_request_id(&root_hash, &resp.request_id)?;

            // Store the epoch in the merkle tree (critical for on-chain verification)
            if let Ok(Some(mut tree)) = storage.get_merkle_tree_by_root(&root_hash) {
                tree.epoch = Some(resp.epoch);
                let _ = storage.save_merkle_tree(&tree);
            }

            tracing::info!(
                root = %root_hash,
                request_id = %resp.request_id,
                epoch = resp.epoch,
                "submitted for signing"
            );
            return Ok(()); // Will poll for proof on next sync cycle
        }

        let request_id = request_id.unwrap();

        // Try to get aggregation proof
        match symbiotic_relay_client.get_aggregation_proof(&request_id).await {
            Ok(resp) => {
                // Success - attach proof to merkle tree
                let mut tree = storage
                    .get_merkle_tree_by_root(&root_hash)?
                    .ok_or(SignerError::TreeNotFound)?;

                if let Some(agg_proof) = resp.aggregation_proof {
                    tree.proof = agg_proof.proof;
                }
                storage.save_merkle_tree(&tree)?;

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
