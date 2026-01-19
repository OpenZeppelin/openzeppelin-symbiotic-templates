use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::B256;
use futures::future::join_all;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;

use alloy::primitives::Address;

use crate::config::AppConfig;
use crate::crypto::{compute_dvn_leaf, encode_signing_message, merkle_root};
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
            let config_clone = Arc::clone(&self.config);
            let symbiotic_relay_client_clone = symbiotic_relay_client.clone();
            let shutdown_rx_worker = shutdown_rx.resubscribe();
            let rx_clone = Arc::clone(&rx);

            let handle = tokio::spawn(async move {
                Self::process_worker(
                    storage_clone,
                    config_clone,
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
        config: Arc<AppConfig>,
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
                &config,
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

        // Sort and dedup leaves (keeping message_ids in sync)
        // Note: We sort by leaf_hash to match DVN contract expectations
        let mut indexed: Vec<(B256, B256)> = leaf_hashes
            .into_iter()
            .zip(valid_msg_ids)
            .collect();
        indexed.sort_by(|a, b| a.0.as_slice().cmp(b.0.as_slice()));
        indexed.dedup_by(|a, b| a.0 == b.0);

        let mut sorted_leaves: Vec<B256> = indexed.iter().map(|(leaf, _)| *leaf).collect();
        let sorted_msg_ids: Vec<B256> = indexed.iter().map(|(_, msg_id)| *msg_id).collect();

        // Handle single-message case by adding zero hash to leaves only (not message_ids)
        // This padding is needed for merkle tree construction but should not be tracked
        // for submission status since B256::ZERO is not a real message
        if sorted_leaves.len() == 1 {
            // Insert B256::ZERO in its sorted position
            let pos = sorted_leaves
                .binary_search_by(|probe| probe.as_slice().cmp(B256::ZERO.as_slice()))
                .unwrap_or_else(|pos| pos);
            sorted_leaves.insert(pos, B256::ZERO);
        }

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
        config: &AppConfig,
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

                // Encode signing message (sidecar will hash internally)
                let signing_message = Self::encode_signing_message_for_tree(config, &tree)?;
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
                    request_id = %resp.request_id,
                    epoch = resp.epoch,
                    "submitted for signing"
                );
                return Ok(());
            }
        };

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

    /// Encode signing message for a merkle tree: abi.encode(chainId, dvnAddress, merkleRoot)
    fn encode_signing_message_for_tree(
        config: &AppConfig,
        tree: &MerkleTreeData,
    ) -> Result<Vec<u8>, SignerError> {
        let dvn_address_str = config
            .layerzero
            .as_ref()
            .and_then(|lz| lz.dvn_addresses.get(&tree.destination_chain))
            .ok_or_else(|| {
                SignerError::EvmClient(format!(
                    "DVN address not configured for chain {}",
                    tree.destination_chain
                ))
            })?;

        let dvn_address: Address = dvn_address_str.parse().map_err(|e| {
            SignerError::EvmClient(format!("invalid DVN address: {}", e))
        })?;

        Ok(encode_signing_message(
            tree.destination_chain,
            dvn_address,
            tree.root_hash,
        ))
    }
}
