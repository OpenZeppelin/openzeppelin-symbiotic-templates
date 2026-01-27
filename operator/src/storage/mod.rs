use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::B256;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::error::StorageError;

/// Get current Unix timestamp in seconds
#[inline]
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}

// Table definitions
const MESSAGES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("messages");
const MERKLE_TREES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("merkle_trees");
const PENDING_PROOFS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("pending_proofs");
const BLOCK_MERKLE_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("block_merkle");
const SUBMISSION_STATUS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("submission_status");
const IDEMPOTENCY_INDEX_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("idempotency_index");
const RELAYER_TX_INDEX_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("relayer_tx_index");
const MESSAGE_STATUS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("message_status");
const MESSAGE_ROOT_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("message_root");

/// Message processing status
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageStatus {
    /// Message received via webhook, awaiting processing
    Pending,
    /// Message is being processed (merkle tree created)
    Processing,
    /// Message has been signed (proof attached)
    Signed,
}

/// Message metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub source_chain: u64,
    pub destination_chain: u64,
    pub block_number: u64,
    pub message_id: B256,
    pub event_tx_hash: B256,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
}

/// Message data with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageData {
    pub metadata: MessageMetadata,
    pub data: Vec<u8>, // Protocol-specific event data (JSON)
}

/// Merkle tree data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleTreeData {
    pub root_hash: B256,
    pub message_ids: Vec<B256>, // Original message IDs for lookup
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub leaf_hashes: Vec<B256>, // DVN-compatible leaf hashes (parallel to message_ids)
    pub source_chain: u64,
    pub destination_chain: u64,
    pub block_numbers: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub proof: Vec<u8>, // BLS proof (empty until signed)
    /// Epoch from the relay's BLS signature response (required for on-chain verification)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub epoch: Option<u64>,
}

/// Submission status for on-chain proof submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionStatus {
    pub message_id: B256,
    pub root_hash: B256,
    pub destination_chain: u64,
    pub status: SubmissionState,
    pub tx_hash: Option<B256>,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// OZ Relayer transaction ID (for tracking via API/webhooks)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub relayer_tx_id: Option<String>,
    /// Idempotency key to prevent double-submission
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub idempotency_key: Option<String>,
}

impl SubmissionStatus {
    /// Create a new pending submission status
    pub fn new_pending(message_id: B256, root_hash: B256, destination_chain: u64) -> Self {
        let now = unix_timestamp();
        Self {
            message_id,
            root_hash,
            destination_chain,
            status: SubmissionState::Pending,
            tx_hash: None,
            retry_count: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
            relayer_tx_id: None,
            idempotency_key: None,
        }
    }

    /// Create a new pending submission status with idempotency key
    pub fn new_pending_with_key(
        message_id: B256,
        root_hash: B256,
        destination_chain: u64,
        idempotency_key: String,
    ) -> Self {
        let mut status = Self::new_pending(message_id, root_hash, destination_chain);
        status.idempotency_key = Some(idempotency_key);
        status
    }

    /// Set the OZ Relayer transaction ID
    pub fn set_relayer_tx_id(&mut self, tx_id: String) {
        self.relayer_tx_id = Some(tx_id);
        self.status = SubmissionState::Submitted;
        self.updated_at = unix_timestamp();
    }

    /// Mark as confirmed with optional transaction hash
    pub fn mark_confirmed(&mut self, tx_hash: Option<B256>) {
        self.status = SubmissionState::Confirmed;
        self.tx_hash = tx_hash;
        self.updated_at = unix_timestamp();
    }

    /// Mark as failed
    pub fn mark_failed(&mut self) {
        self.status = SubmissionState::Failed;
        self.updated_at = unix_timestamp();
    }

}

/// State of a submission
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubmissionState {
    Pending,
    Submitted,
    Confirmed,
    Failed,
}

/// Storage layer using redb
pub struct Storage {
    db: Database,
}

impl Storage {
    /// Create a new storage instance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }

        let db = Database::create(path.as_ref())?;

        // Initialize tables
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(MESSAGES_TABLE)?;
            let _ = write_txn.open_table(MERKLE_TREES_TABLE)?;
            let _ = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            let _ = write_txn.open_table(BLOCK_MERKLE_TABLE)?;
            let _ = write_txn.open_table(SUBMISSION_STATUS_TABLE)?;
            let _ = write_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;
            let _ = write_txn.open_table(RELAYER_TX_INDEX_TABLE)?;
            let _ = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            let _ = write_txn.open_table(MESSAGE_ROOT_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }

    /// Save a message (idempotent - duplicates are ignored)
    pub fn save_message(&self, msg: &MessageData) -> Result<(), StorageError> {
        let key = Self::message_key(&msg.metadata.message_id);
        let value = serde_json::to_vec(msg)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGES_TABLE)?;

            // Check if key exists (for idempotency)
            if table.get(key.as_slice())?.is_some() {
                tracing::debug!(
                    message_id = %msg.metadata.message_id,
                    "duplicate message ignored"
                );
                return Ok(());
            }

            table.insert(key.as_slice(), value.as_slice())?;

            // Set initial message status to Pending
            let mut status_table = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            let status_key = Self::message_status_key(&msg.metadata.message_id);
            let status_value = serde_json::to_vec(&MessageStatus::Pending)?;
            status_table.insert(status_key.as_slice(), status_value.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    /// Get a message by ID
    pub fn get_message(&self, id: &B256) -> Result<Option<MessageData>, StorageError> {
        let key = Self::message_key(id);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MESSAGES_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

    /// List messages by status
    pub fn list_messages_by_status(
        &self,
        status: MessageStatus,
    ) -> Result<Vec<MessageData>, StorageError> {
        Ok(self
            .list_messages_with_status_filter(Some(status))?
            .into_iter()
            .map(|(msg, _)| msg)
            .collect())
    }

    /// Update message status
    pub fn update_message_status(
        &self,
        id: &B256,
        status: MessageStatus,
    ) -> Result<(), StorageError> {
        let key = Self::message_status_key(id);
        let value = serde_json::to_vec(&status)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            table.insert(key.as_slice(), value.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    /// List all messages with their status (for debug API)
    pub fn list_all_messages_with_status(
        &self,
    ) -> Result<Vec<(MessageData, MessageStatus)>, StorageError> {
        self.list_messages_with_status_filter(None)
    }

    /// Internal helper to list messages with optional status filter
    fn list_messages_with_status_filter(
        &self,
        filter_status: Option<MessageStatus>,
    ) -> Result<Vec<(MessageData, MessageStatus)>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let messages_table = read_txn.open_table(MESSAGES_TABLE)?;
        let status_table = read_txn.open_table(MESSAGE_STATUS_TABLE)?;

        let mut results = Vec::new();
        let msg_prefix = b"msg:";

        for result in messages_table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();

            if key_bytes.starts_with(msg_prefix) && key_bytes.len() == msg_prefix.len() + 32 {
                // Get status for this message
                let msg_id = B256::from_slice(&key_bytes[msg_prefix.len()..]);
                let status_key = Self::message_status_key(&msg_id);
                let status = match status_table.get(status_key.as_slice())? {
                    Some(v) => serde_json::from_slice(v.value())?,
                    None => {
                        tracing::warn!(
                            message_id = %msg_id,
                            "message exists but status entry missing, defaulting to Pending"
                        );
                        MessageStatus::Pending
                    }
                };

                // Apply filter if specified
                if filter_status.is_none() || filter_status == Some(status) {
                    let msg: MessageData = serde_json::from_slice(value.value())?;
                    results.push((msg, status));
                }
            }
        }

        Ok(results)
    }

    /// Save merkle tree
    pub fn save_merkle_tree(&self, tree: &MerkleTreeData) -> Result<(), StorageError> {
        let key = Self::merkle_key(&tree.root_hash);
        let value = serde_json::to_vec(tree)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MERKLE_TREES_TABLE)?;
            table.insert(key.as_slice(), value.as_slice())?;

            // Manage pending entry based on proof status
            let mut pending_table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            let pending_key = Self::pending_key(&tree.root_hash);

            if tree.proof.is_empty() {
                // Only create pending entry if one doesn't exist yet
                // This preserves any existing request_id from set_pending_request_id()
                if pending_table.get(pending_key.as_slice())?.is_none() {
                    pending_table.insert(pending_key.as_slice(), &[] as &[u8])?;
                }
            } else {
                // Tree has proof - remove from pending
                let _ = pending_table.remove(pending_key.as_slice());
            }

            // Create block -> merkle lookup for each block
            let mut block_table = write_txn.open_table(BLOCK_MERKLE_TABLE)?;
            for &block_num in &tree.block_numbers {
                let block_key =
                    Self::block_merkle_key(tree.source_chain, tree.destination_chain, block_num);
                block_table.insert(block_key.as_slice(), tree.root_hash.as_slice())?;
            }

            // Create message_id -> root_hash lookup for each message
            let mut msg_root_table = write_txn.open_table(MESSAGE_ROOT_TABLE)?;
            for msg_id in &tree.message_ids {
                // Skip zero hash (padding for single-message trees)
                if *msg_id != B256::ZERO {
                    let msg_root_key = Self::message_root_key(msg_id);
                    msg_root_table.insert(msg_root_key.as_slice(), tree.root_hash.as_slice())?;
                }
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    /// Get merkle tree by root hash
    pub fn get_merkle_tree_by_root(
        &self,
        root: &B256,
    ) -> Result<Option<MerkleTreeData>, StorageError> {
        let key = Self::merkle_key(root);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MERKLE_TREES_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

    /// Get merkle root hash by message ID
    pub fn get_merkle_root_by_message(&self, message_id: &B256) -> Result<Option<B256>, StorageError> {
        let key = Self::message_root_key(message_id);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MESSAGE_ROOT_TABLE)?;

        Ok(table
            .get(key.as_slice())?
            .map(|v| B256::from_slice(v.value())))
    }

    /// List all pending merkle roots with their request IDs (if any)
    pub fn list_pending_merkle_roots(&self) -> Result<HashMap<B256, Option<String>>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        let mut roots = HashMap::new();
        let prefix = b"merklependingproof:";

        for result in table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();
            let value_bytes = value.value();

            if key_bytes.starts_with(prefix) && key_bytes.len() == prefix.len() + 32 {
                let root = B256::from_slice(&key_bytes[prefix.len()..]);
                let request_id = if value_bytes.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(value_bytes).to_string())
                };
                roots.insert(root, request_id);
            }
        }

        Ok(roots)
    }

    /// Get pending request ID for a root hash
    pub fn get_pending_request_id(&self, root: &B256) -> Result<Option<String>, StorageError> {
        let key = Self::pending_key(root);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PENDING_PROOFS_TABLE)?;

        match table.get(key.as_slice())? {
            Some(value) => {
                let value_bytes = value.value();
                if value_bytes.is_empty() {
                    Ok(None) // Pending but no request ID yet
                } else {
                    let request_id = String::from_utf8_lossy(value_bytes).to_string();
                    Ok(Some(request_id))
                }
            }
            None => Ok(None),
        }
    }

    /// Set request ID for a pending root hash
    pub fn set_pending_request_id(
        &self,
        root: &B256,
        request_id: &str,
    ) -> Result<(), StorageError> {
        let key = Self::pending_key(root);

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            table.insert(key.as_slice(), request_id.as_bytes())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    /// Delete pending entry
    pub fn delete_pending(&self, root: &B256) -> Result<(), StorageError> {
        let key = Self::pending_key(root);

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PENDING_PROOFS_TABLE)?;
            table.remove(key.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    // Key generation helpers

    #[inline]
    fn prefix_key(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
        [prefix, suffix].concat()
    }

    fn message_key(id: &B256) -> Vec<u8> {
        Self::prefix_key(b"msg:", id.as_slice())
    }

    fn merkle_key(root: &B256) -> Vec<u8> {
        Self::prefix_key(b"merkle:", root.as_slice())
    }

    fn pending_key(root: &B256) -> Vec<u8> {
        Self::prefix_key(b"merklependingproof:", root.as_slice())
    }

    fn block_merkle_key(src: u64, dst: u64, block: u64) -> Vec<u8> {
        // "blockmerkle:" (12) + u64 (8) + ":" (1) + u64 (8) + ":" (1) + u64 (8) = 38 bytes
        let mut key = Vec::with_capacity(38);
        key.extend_from_slice(b"blockmerkle:");
        key.extend_from_slice(&src.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(&dst.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(&block.to_be_bytes());
        key
    }

    fn message_status_key(id: &B256) -> Vec<u8> {
        Self::prefix_key(b"msgstatus:", id.as_slice())
    }

    fn message_root_key(id: &B256) -> Vec<u8> {
        Self::prefix_key(b"msgroot:", id.as_slice())
    }

    fn submission_status_key(chain_id: u64, message_id: &B256) -> Vec<u8> {
        // "submission:" (11) + u64 (8) + ":" (1) + B256 (32) = 52 bytes
        let mut key = Vec::with_capacity(52);
        key.extend_from_slice(b"submission:");
        key.extend_from_slice(&chain_id.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(message_id.as_slice());
        key
    }

    // Submission status tracking methods

    /// Save submission status (also maintains index tables for lookups)
    pub fn save_submission_status(&self, status: &SubmissionStatus) -> Result<(), StorageError> {
        let key = Self::submission_status_key(status.destination_chain, &status.message_id);
        let value = serde_json::to_vec(status)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(SUBMISSION_STATUS_TABLE)?;
            table.insert(key.as_slice(), value.as_slice())?;

            // Update idempotency key index if present
            if let Some(ref idem_key) = status.idempotency_key {
                let mut idem_table = write_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;
                idem_table.insert(idem_key.as_bytes(), key.as_slice())?;
            }

            // Update relayer tx ID index if present
            if let Some(ref tx_id) = status.relayer_tx_id {
                let mut tx_table = write_txn.open_table(RELAYER_TX_INDEX_TABLE)?;
                tx_table.insert(tx_id.as_bytes(), key.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Get submission status by chain and message ID
    pub fn get_submission_status(
        &self,
        chain_id: u64,
        message_id: &B256,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let key = Self::submission_status_key(chain_id, message_id);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

    /// List signed merkle trees that haven't had all submissions completed
    pub fn list_signed_trees_without_submissions(&self) -> Result<Vec<MerkleTreeData>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let trees_table = read_txn.open_table(MERKLE_TREES_TABLE)?;
        let submissions_table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        let mut trees = Vec::new();
        let merkle_prefix = b"merkle:";

        for result in trees_table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();

            if key_bytes.starts_with(merkle_prefix) {
                let tree: MerkleTreeData = serde_json::from_slice(value.value())?;

                // Only include signed trees (non-empty proof)
                if tree.proof.is_empty() {
                    continue;
                }

                // Check if any message needs submission
                let mut needs_submission = false;
                for msg_id in &tree.message_ids {
                    let sub_key =
                        Self::submission_status_key(tree.destination_chain, msg_id);
                    match submissions_table.get(sub_key.as_slice())? {
                        Some(v) => {
                            let status: SubmissionStatus = serde_json::from_slice(v.value())?;
                            if status.status != SubmissionState::Confirmed {
                                needs_submission = true;
                                break;
                            }
                        }
                        None => {
                            needs_submission = true;
                            break;
                        }
                    }
                }

                if needs_submission {
                    trees.push(tree);
                }
            }
        }

        Ok(trees)
    }

    /// Get submission status by idempotency key
    pub fn get_submission_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let idem_table = read_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;

        match idem_table.get(key.as_bytes())? {
            Some(status_key) => {
                let submissions_table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;
                submissions_table
                    .get(status_key.value())?
                    .map(|v| serde_json::from_slice(v.value()))
                    .transpose()
                    .map_err(Into::into)
            }
            None => Ok(None),
        }
    }

    /// Get submission status by OZ Relayer transaction ID
    pub fn get_submission_by_relayer_tx_id(
        &self,
        tx_id: &str,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let tx_table = read_txn.open_table(RELAYER_TX_INDEX_TABLE)?;

        match tx_table.get(tx_id.as_bytes())? {
            Some(status_key) => {
                let submissions_table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;
                submissions_table
                    .get(status_key.value())?
                    .map(|v| serde_json::from_slice(v.value()))
                    .transpose()
                    .map_err(Into::into)
            }
            None => Ok(None),
        }
    }

    /// List submissions that have a relayer tx ID but are not yet confirmed (for status polling)
    pub fn list_pending_relayer_submissions(&self) -> Result<Vec<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        let mut submissions = Vec::new();
        let prefix = b"submission:";

        for result in table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();

            if key_bytes.starts_with(prefix) {
                let status: SubmissionStatus = serde_json::from_slice(value.value())?;
                // Include if has relayer tx ID and not yet terminal
                if status.relayer_tx_id.is_some()
                    && !matches!(
                        status.status,
                        SubmissionState::Confirmed | SubmissionState::Failed
                    )
                {
                    submissions.push(status);
                }
            }
        }

        Ok(submissions)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    // Helper to create a test message
    fn test_message(id: B256, src: u64, dst: u64, block: u64) -> MessageData {
        MessageData {
            metadata: MessageMetadata {
                source_chain: src,
                destination_chain: dst,
                block_number: block,
                message_id: id,
                event_tx_hash: B256::from_slice(&[0x02u8; 32]),
                ttl: None,
            },
            data: b"test data".to_vec(),
        }
    }

    #[test]
    fn test_storage_create() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        drop(storage);
    }

    #[test]
    fn test_storage_create_with_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/dir/test.db");
        let storage = Storage::new(&path).unwrap();
        drop(storage);
        assert!(path.exists());
    }

    #[test]
    fn test_save_and_get_message() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 42161,
                block_number: 12345,
                message_id: B256::from_slice(&[1u8; 32]),
                event_tx_hash: B256::from_slice(&[2u8; 32]),
                ttl: None,
            },
            data: b"test data".to_vec(),
        };

        storage.save_message(&msg).unwrap();

        let retrieved = storage
            .get_message(&msg.metadata.message_id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.metadata.source_chain, msg.metadata.source_chain);
        assert_eq!(retrieved.metadata.block_number, msg.metadata.block_number);
    }

    #[test]
    fn test_idempotent_save() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 42161,
                block_number: 12345,
                message_id: B256::from_slice(&[1u8; 32]),
                event_tx_hash: B256::from_slice(&[2u8; 32]),
                ttl: None,
            },
            data: b"test data".to_vec(),
        };

        // Save twice - should not error
        storage.save_message(&msg).unwrap();
        storage.save_message(&msg).unwrap();
    }

    #[test]
    fn test_submission_status_idempotency_key_lookup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let message_id = B256::from_slice(&[0x11u8; 32]);
        let root_hash = B256::from_slice(&[0x22u8; 32]);
        let chain_id = 42161u64;
        let idem_key = "bg-test-key-123".to_string();

        // Initially, no entry exists
        assert!(storage
            .get_submission_by_idempotency_key(&idem_key)
            .unwrap()
            .is_none());

        // Create a pending status with idempotency key (no relayer_tx_id yet)
        let status = SubmissionStatus::new_pending_with_key(
            message_id,
            root_hash,
            chain_id,
            idem_key.clone(),
        );
        storage.save_submission_status(&status).unwrap();

        // Entry should be found even without relayer_tx_id
        let found = storage
            .get_submission_by_idempotency_key(&idem_key)
            .unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.message_id, message_id);
        assert!(found.relayer_tx_id.is_none()); // No relayer_tx_id yet
        assert_eq!(found.status, SubmissionState::Pending);
    }

    #[test]
    fn test_submission_status_state_transitions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let message_id = B256::from_slice(&[0x33u8; 32]);
        let root_hash = B256::from_slice(&[0x44u8; 32]);
        let chain_id = 42161u64;

        // Create pending status
        let mut status = SubmissionStatus::new_pending(message_id, root_hash, chain_id);
        storage.save_submission_status(&status).unwrap();

        // Verify pending state
        let retrieved = storage
            .get_submission_status(chain_id, &message_id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.status, SubmissionState::Pending);

        // Transition to Submitted
        status.set_relayer_tx_id("relayer-tx-123".to_string());
        storage.save_submission_status(&status).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &message_id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.status, SubmissionState::Submitted);
        assert_eq!(
            retrieved.relayer_tx_id,
            Some("relayer-tx-123".to_string())
        );

        // Transition to Confirmed
        status.mark_confirmed(Some(B256::from_slice(&[0x55u8; 32])));
        storage.save_submission_status(&status).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &message_id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.status, SubmissionState::Confirmed);
        assert!(retrieved.tx_hash.is_some());
    }

    #[test]
    fn test_submission_dedup_skip_existing_entry() {
        // Test that deduplication works when any entry exists (regardless of relayer_tx_id)
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let message_id = B256::from_slice(&[0x66u8; 32]);
        let root_hash = B256::from_slice(&[0x77u8; 32]);
        let chain_id = 42161u64;
        let idem_key = "bg-dedup-test".to_string();

        // Simulate first submission creating a Pending entry (no relayer_tx_id yet)
        let status = SubmissionStatus::new_pending_with_key(
            message_id,
            root_hash,
            chain_id,
            idem_key.clone(),
        );
        storage.save_submission_status(&status).unwrap();

        // Second submission should find existing entry and skip
        // This is the key test: entry exists but has no relayer_tx_id
        let existing = storage.get_submission_by_idempotency_key(&idem_key).unwrap();
        assert!(
            existing.is_some(),
            "Should find entry even without relayer_tx_id"
        );
    }

    #[test]
    fn test_submission_dedup_skip_non_pending_states() {
        // Test that all non-Pending states trigger skip
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let chain_id = 42161u64;

        // Test Submitted state
        let msg_id_1 = B256::from_slice(&[0x01u8; 32]);
        let mut status1 =
            SubmissionStatus::new_pending(msg_id_1, B256::ZERO, chain_id);
        status1.set_relayer_tx_id("tx-1".to_string());
        storage.save_submission_status(&status1).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_id_1)
            .unwrap()
            .unwrap();
        assert_ne!(
            retrieved.status,
            SubmissionState::Pending,
            "Submitted should trigger skip"
        );

        // Test Confirmed state
        let msg_id_2 = B256::from_slice(&[0x02u8; 32]);
        let mut status2 =
            SubmissionStatus::new_pending(msg_id_2, B256::ZERO, chain_id);
        status2.mark_confirmed(None);
        storage.save_submission_status(&status2).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_id_2)
            .unwrap()
            .unwrap();
        assert_ne!(
            retrieved.status,
            SubmissionState::Pending,
            "Confirmed should trigger skip"
        );

        // Test Failed state
        let msg_id_3 = B256::from_slice(&[0x03u8; 32]);
        let mut status3 =
            SubmissionStatus::new_pending(msg_id_3, B256::ZERO, chain_id);
        status3.mark_failed();
        storage.save_submission_status(&status3).unwrap();

        let retrieved = storage
            .get_submission_status(chain_id, &msg_id_3)
            .unwrap()
            .unwrap();
        assert_ne!(
            retrieved.status,
            SubmissionState::Pending,
            "Failed should trigger skip"
        );
    }

    #[test]
    fn test_message_root_lookup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let leaf_hash = B256::from_slice(&[0x22u8; 32]);
        let root_hash = B256::from_slice(&[0x33u8; 32]);

        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![msg_id],
            leaf_hashes: vec![leaf_hash],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };

        storage.save_merkle_tree(&tree).unwrap();

        // Should find root by message_id
        let found = storage.get_merkle_root_by_message(&msg_id).unwrap();
        assert_eq!(found, Some(root_hash));

        // B256::ZERO should not be indexed (padding for single-message trees)
        let not_found = storage.get_merkle_root_by_message(&B256::ZERO).unwrap();
        assert!(not_found.is_none());

        // Non-existent message should return None
        let unknown = B256::from_slice(&[0x99u8; 32]);
        let not_found = storage.get_merkle_root_by_message(&unknown).unwrap();
        assert!(not_found.is_none());
    }

    // ============ Phase 1: Additional Storage Tests ============

    #[test]
    fn test_list_messages_by_status_pending() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Save messages (all start as Pending)
        let msg1 = test_message(B256::from_slice(&[0x01u8; 32]), 1, 42161, 100);
        let msg2 = test_message(B256::from_slice(&[0x02u8; 32]), 1, 42161, 101);
        storage.save_message(&msg1).unwrap();
        storage.save_message(&msg2).unwrap();

        let pending = storage.list_messages_by_status(MessageStatus::Pending).unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn test_list_messages_by_status_processing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg1 = test_message(B256::from_slice(&[0x01u8; 32]), 1, 42161, 100);
        let msg2 = test_message(B256::from_slice(&[0x02u8; 32]), 1, 42161, 101);
        storage.save_message(&msg1).unwrap();
        storage.save_message(&msg2).unwrap();

        // Update one to Processing
        storage
            .update_message_status(&msg1.metadata.message_id, MessageStatus::Processing)
            .unwrap();

        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(processing.len(), 1);
        assert_eq!(processing[0].metadata.message_id, msg1.metadata.message_id);

        let pending = storage.list_messages_by_status(MessageStatus::Pending).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_list_messages_by_status_signed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg = test_message(B256::from_slice(&[0x01u8; 32]), 1, 42161, 100);
        storage.save_message(&msg).unwrap();
        storage
            .update_message_status(&msg.metadata.message_id, MessageStatus::Signed)
            .unwrap();

        let signed = storage.list_messages_by_status(MessageStatus::Signed).unwrap();
        assert_eq!(signed.len(), 1);
    }

    #[test]
    fn test_list_messages_by_status_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let pending = storage.list_messages_by_status(MessageStatus::Pending).unwrap();
        assert!(pending.is_empty());

        let processing = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert!(processing.is_empty());
    }

    #[test]
    fn test_list_all_messages_with_status() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg1 = test_message(B256::from_slice(&[0x01u8; 32]), 1, 42161, 100);
        let msg2 = test_message(B256::from_slice(&[0x02u8; 32]), 1, 42161, 101);
        let msg3 = test_message(B256::from_slice(&[0x03u8; 32]), 1, 42161, 102);
        storage.save_message(&msg1).unwrap();
        storage.save_message(&msg2).unwrap();
        storage.save_message(&msg3).unwrap();

        storage
            .update_message_status(&msg2.metadata.message_id, MessageStatus::Processing)
            .unwrap();
        storage
            .update_message_status(&msg3.metadata.message_id, MessageStatus::Signed)
            .unwrap();

        let all = storage.list_all_messages_with_status().unwrap();
        assert_eq!(all.len(), 3);

        // Verify statuses are correct
        for (msg, status) in &all {
            match msg.metadata.message_id.0[0] {
                0x01 => assert_eq!(*status, MessageStatus::Pending),
                0x02 => assert_eq!(*status, MessageStatus::Processing),
                0x03 => assert_eq!(*status, MessageStatus::Signed),
                _ => panic!("unexpected message"),
            }
        }
    }

    #[test]
    fn test_get_merkle_tree_by_root_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![B256::from_slice(&[0x01u8; 32])],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32])],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![100],
            proof: vec![],
            epoch: None,
        };

        storage.save_merkle_tree(&tree).unwrap();

        let found = storage.get_merkle_tree_by_root(&root).unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.root_hash, root);
        assert_eq!(found.source_chain, 1);
    }

    #[test]
    fn test_get_merkle_tree_by_root_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let found = storage.get_merkle_tree_by_root(&root).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_list_signed_trees_without_submissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create a signed tree (has proof)
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32])],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![100],
            proof: vec![0u8; 96], // Non-empty proof = signed
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Should appear in list (no submission status yet)
        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert_eq!(trees.len(), 1);
    }

    #[test]
    fn test_list_signed_trees_excludes_unsigned() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Create an unsigned tree (empty proof)
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![B256::from_slice(&[0x01u8; 32])],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32])],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![100],
            proof: vec![], // Empty proof = not signed
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert!(trees.is_empty(), "unsigned trees should not appear");
    }

    #[test]
    fn test_list_signed_trees_excludes_confirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root = B256::from_slice(&[0xAAu8; 32]);
        let chain_id = 42161u64;

        // Create a signed tree
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32])],
            source_chain: 1,
            destination_chain: chain_id,
            block_numbers: vec![100],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Create confirmed submission status
        let mut status = SubmissionStatus::new_pending(msg_id, root, chain_id);
        status.mark_confirmed(Some(B256::from_slice(&[0xBBu8; 32])));
        storage.save_submission_status(&status).unwrap();

        // Should NOT appear (all messages confirmed)
        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert!(trees.is_empty(), "confirmed trees should not appear");
    }

    #[test]
    fn test_pending_merkle_root_lifecycle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create unsigned tree (adds to pending)
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![B256::from_slice(&[0x01u8; 32])],
            leaf_hashes: vec![B256::from_slice(&[0x11u8; 32])],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Should be in pending without request ID
        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(pending.contains_key(&root));
        assert!(pending.get(&root).unwrap().is_none());

        // Set request ID
        storage.set_pending_request_id(&root, "req-123").unwrap();

        // Should now have request ID
        let req_id = storage.get_pending_request_id(&root).unwrap();
        assert_eq!(req_id, Some("req-123".to_string()));

        // Delete pending
        storage.delete_pending(&root).unwrap();

        // Should no longer be in pending
        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(!pending.contains_key(&root));
    }

    #[test]
    fn test_list_pending_merkle_roots() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Create multiple pending trees
        for i in 0..3 {
            let root = B256::from_slice(&[i + 1; 32]);
            let tree = MerkleTreeData {
                root_hash: root,
                message_ids: vec![],
                leaf_hashes: vec![],
                source_chain: 1,
                destination_chain: 42161,
                block_numbers: vec![],
                proof: vec![],
                epoch: None,
            };
            storage.save_merkle_tree(&tree).unwrap();
        }

        // Set request ID for one
        let root_with_id = B256::from_slice(&[1; 32]);
        storage.set_pending_request_id(&root_with_id, "req-1").unwrap();

        let pending = storage.list_pending_merkle_roots().unwrap();
        assert_eq!(pending.len(), 3);
        assert_eq!(pending.get(&root_with_id).unwrap(), &Some("req-1".to_string()));
    }

    #[test]
    fn test_delete_pending_edge_cases() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);

        // Delete non-existent pending should not error
        storage.delete_pending(&root).unwrap();

        // Verify still empty
        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(!pending.contains_key(&root));
    }

    #[test]
    fn test_get_submission_by_idempotency_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let root = B256::from_slice(&[0x22u8; 32]);
        let chain_id = 42161u64;
        let idem_key = "test-idem-key".to_string();

        // Create submission with idempotency key
        let status = SubmissionStatus::new_pending_with_key(msg_id, root, chain_id, idem_key.clone());
        storage.save_submission_status(&status).unwrap();

        // Should find by idempotency key
        let found = storage.get_submission_by_idempotency_key(&idem_key).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().message_id, msg_id);

        // Non-existent key returns None
        let not_found = storage.get_submission_by_idempotency_key("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_get_submission_by_relayer_tx_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let root = B256::from_slice(&[0x22u8; 32]);
        let chain_id = 42161u64;
        let tx_id = "relayer-tx-abc123".to_string();

        // Create submission and set relayer tx ID
        let mut status = SubmissionStatus::new_pending(msg_id, root, chain_id);
        status.set_relayer_tx_id(tx_id.clone());
        storage.save_submission_status(&status).unwrap();

        // Should find by relayer tx ID
        let found = storage.get_submission_by_relayer_tx_id(&tx_id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().message_id, msg_id);

        // Non-existent returns None
        let not_found = storage.get_submission_by_relayer_tx_id("nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_list_pending_relayer_submissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let chain_id = 42161u64;

        // Create submission without relayer tx ID (should not appear)
        let status1 = SubmissionStatus::new_pending(
            B256::from_slice(&[0x01u8; 32]),
            B256::ZERO,
            chain_id,
        );
        storage.save_submission_status(&status1).unwrap();

        // Create submission with relayer tx ID but Submitted state
        let mut status2 = SubmissionStatus::new_pending(
            B256::from_slice(&[0x02u8; 32]),
            B256::ZERO,
            chain_id,
        );
        status2.set_relayer_tx_id("tx-2".to_string());
        storage.save_submission_status(&status2).unwrap();

        // Create confirmed submission (should not appear)
        let mut status3 = SubmissionStatus::new_pending(
            B256::from_slice(&[0x03u8; 32]),
            B256::ZERO,
            chain_id,
        );
        status3.set_relayer_tx_id("tx-3".to_string());
        status3.mark_confirmed(None);
        storage.save_submission_status(&status3).unwrap();

        let pending = storage.list_pending_relayer_submissions().unwrap();
        // Only status2 should appear (has tx ID, is Submitted, not terminal)
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].relayer_tx_id,
            Some("tx-2".to_string())
        );
    }

    #[test]
    fn test_submission_status_mark_confirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let tx_hash = B256::from_slice(&[0xAAu8; 32]);
        let chain_id = 42161u64;

        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, chain_id);
        status.mark_confirmed(Some(tx_hash));
        storage.save_submission_status(&status).unwrap();

        let retrieved = storage.get_submission_status(chain_id, &msg_id).unwrap().unwrap();
        assert_eq!(retrieved.status, SubmissionState::Confirmed);
        assert_eq!(retrieved.tx_hash, Some(tx_hash));
    }

    #[test]
    fn test_submission_status_mark_failed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x11u8; 32]);
        let chain_id = 42161u64;

        let mut status = SubmissionStatus::new_pending(msg_id, B256::ZERO, chain_id);
        status.mark_failed();
        storage.save_submission_status(&status).unwrap();

        let retrieved = storage.get_submission_status(chain_id, &msg_id).unwrap().unwrap();
        assert_eq!(retrieved.status, SubmissionState::Failed);
    }

    #[test]
    fn test_get_message_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x99u8; 32]);
        let result = storage.get_message(&msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update_message_status_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg = test_message(B256::from_slice(&[0x01u8; 32]), 1, 42161, 100);
        storage.save_message(&msg).unwrap();

        // Update status multiple times
        storage
            .update_message_status(&msg.metadata.message_id, MessageStatus::Processing)
            .unwrap();
        storage
            .update_message_status(&msg.metadata.message_id, MessageStatus::Processing)
            .unwrap();

        // Should still be processing
        let messages = storage
            .list_messages_by_status(MessageStatus::Processing)
            .unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_merkle_tree_preserves_request_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create unsigned tree
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Set request ID
        storage.set_pending_request_id(&root, "req-123").unwrap();

        // Save tree again (should preserve request ID)
        storage.save_merkle_tree(&tree).unwrap();

        // Request ID should still be there
        let req_id = storage.get_pending_request_id(&root).unwrap();
        assert_eq!(req_id, Some("req-123".to_string()));
    }

    #[test]
    fn test_signed_tree_removes_from_pending() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);

        // Create unsigned tree
        let mut tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 42161,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Should be pending
        assert!(storage.list_pending_merkle_roots().unwrap().contains_key(&root));

        // Add proof (sign the tree)
        tree.proof = vec![0u8; 96];
        tree.epoch = Some(1);
        storage.save_merkle_tree(&tree).unwrap();

        // Should no longer be pending
        assert!(!storage.list_pending_merkle_roots().unwrap().contains_key(&root));
    }
}
