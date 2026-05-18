use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::B256;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::error::StorageError;

#[inline]
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}

// Core tables (provider-scoped keys)
const MESSAGES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("messages");
const MESSAGE_STATUS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("message_status");
const PROVIDER_ARTIFACTS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("provider_artifacts");
const ARTIFACT_BY_MESSAGE_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("artifact_by_message");
const SUBMISSION_STATUS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("submission_status");
const IDEMPOTENCY_INDEX_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("idempotency_index");
const RELAYER_TX_INDEX_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("relayer_tx_index");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageStatus {
    Pending,
    Processing,
    Signed,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageData {
    pub metadata: MessageMetadata,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleTreeData {
    pub root_hash: B256,
    pub message_ids: Vec<B256>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub leaf_hashes: Vec<B256>,
    pub source_chain: u64,
    pub destination_chain: u64,
    pub block_numbers: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub proof: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionStatus {
    pub message_id: B256,
    pub root_hash: B256,
    pub destination_chain: u64,
    /// State of *this operator's own* OZ Relayer submission. Reflects whether
    /// our tx mined; does NOT necessarily reflect whether the message was
    /// delivered to the receiver. See [`execution_state`] for that.
    pub status: SubmissionState,
    pub tx_hash: Option<B256>,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub relayer_tx_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub idempotency_key: Option<String>,
    /// On-chain message-level execution state from OffRamp.ExecutionStateChanged,
    /// populated when an OZ Monitor webhook for that event is received. This is
    /// the authoritative answer to "did the message deliver?" — it's set
    /// independently of which operator's tx mined (peer races) and reflects
    /// whether the receiver's callback succeeded (which the outer tx status does not).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub execution_state: Option<ExecutionState>,
    /// Tx that drove the on-chain execution state change. May differ from
    /// [`tx_hash`] when a peer operator's submission landed first.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub delivery_tx_hash: Option<B256>,
}

impl SubmissionStatus {
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
            execution_state: None,
            delivery_tx_hash: None,
        }
    }

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

    pub fn set_relayer_tx_id(&mut self, tx_id: String) {
        self.relayer_tx_id = Some(tx_id);
        self.status = SubmissionState::Submitted;
        self.updated_at = unix_timestamp();
    }

    pub fn mark_confirmed(&mut self, tx_hash: Option<B256>) {
        self.status = SubmissionState::Confirmed;
        self.tx_hash = tx_hash;
        self.updated_at = unix_timestamp();
    }

    pub fn mark_failed(&mut self) {
        self.status = SubmissionState::Failed;
        self.updated_at = unix_timestamp();
    }

    /// Mark this submission as deduplicated: its leaf hash matched another
    /// message in the same batch, so that message's on-chain transaction
    /// covers both. No relayer transaction is sent for this message.
    pub fn mark_deduplicated(&mut self, primary_message_id: B256) {
        self.status = SubmissionState::Deduplicated;
        self.last_error = Some(format!("deduplicated via {primary_message_id}"));
        self.updated_at = unix_timestamp();
    }

    /// Record the on-chain message-level execution outcome from OffRamp.
    /// Idempotent and authoritative — once Success or Failure is observed it
    /// represents the protocol's final word, regardless of which operator's
    /// tx mined or what our local [`status`] says.
    pub fn set_execution_state(&mut self, state: ExecutionState, tx_hash: B256) {
        self.execution_state = Some(state);
        self.delivery_tx_hash = Some(tx_hash);
        self.updated_at = unix_timestamp();
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubmissionState {
    Pending,
    Submitted,
    Confirmed,
    Failed,
    /// A duplicate-leaf shadow: another message in the same batch hashes to
    /// the same leaf, and its transaction covers this one on-chain.
    Deduplicated,
}

/// On-chain message-level execution outcome from CCIP OffRamp's
/// ExecutionStateChanged event. Mirrors `Internal.MessageExecutionState`:
/// Untouched=0, InProgress=1, Success=2, Failure=3. We only persist terminal
/// values — Success means the receiver's callback completed; Failure means the
/// callback reverted and the message can be re-executed via `manuallyExecute`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionState {
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderArtifact {
    pub artifact_id: String,
    pub kind: String,
    pub source_chain: u64,
    pub destination_chain: u64,
    pub message_ids: Vec<B256>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub root_hash: Option<B256>,
    pub payload: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_request_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl ProviderArtifact {
    fn new_merkle(
        tree: &MerkleTreeData,
        pending_request_id: Option<String>,
    ) -> Result<Self, StorageError> {
        let now = unix_timestamp();
        Ok(Self {
            artifact_id: tree.root_hash.to_string(),
            kind: "merkle_tree_v1".to_string(),
            source_chain: tree.source_chain,
            destination_chain: tree.destination_chain,
            message_ids: tree.message_ids.clone(),
            root_hash: Some(tree.root_hash),
            payload: serde_json::to_vec(tree)?,
            pending_request_id,
            created_at: now,
            updated_at: now,
        })
    }

    fn as_merkle_tree(&self) -> Result<MerkleTreeData, StorageError> {
        Ok(serde_json::from_slice(&self.payload)?)
    }

    fn is_merkle_kind(&self) -> bool {
        self.kind == "merkle_tree_v1"
    }
}

pub struct Storage {
    db: Database,
    provider: String,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        Self::new_with_provider(path, "default")
    }

    pub fn new_with_provider<P: AsRef<Path>>(
        path: P,
        provider: &str,
    ) -> Result<Self, StorageError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }

        let provider = provider.trim().to_lowercase();
        if provider.is_empty() {
            return Err(StorageError::NotFound(
                "provider cannot be empty".to_string(),
            ));
        }

        let db = Database::create(path.as_ref())?;

        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(MESSAGES_TABLE)?;
            let _ = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            let _ = write_txn.open_table(PROVIDER_ARTIFACTS_TABLE)?;
            let _ = write_txn.open_table(ARTIFACT_BY_MESSAGE_TABLE)?;
            let _ = write_txn.open_table(SUBMISSION_STATUS_TABLE)?;
            let _ = write_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;
            let _ = write_txn.open_table(RELAYER_TX_INDEX_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db, provider })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn save_message(&self, msg: &MessageData) -> Result<(), StorageError> {
        let key = self.message_key(&msg.metadata.message_id);
        let value = serde_json::to_vec(msg)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGES_TABLE)?;
            if table.get(key.as_slice())?.is_some() {
                tracing::debug!(message_id = %msg.metadata.message_id, provider = %self.provider, "duplicate message ignored");
                return Ok(());
            }

            table.insert(key.as_slice(), value.as_slice())?;

            let mut status_table = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            let status_key = self.message_status_key(&msg.metadata.message_id);
            let status_value = serde_json::to_vec(&MessageStatus::Pending)?;
            status_table.insert(status_key.as_slice(), status_value.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub fn get_message(&self, id: &B256) -> Result<Option<MessageData>, StorageError> {
        let key = self.message_key(id);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(MESSAGES_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

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

    pub fn update_message_status(
        &self,
        id: &B256,
        status: MessageStatus,
    ) -> Result<(), StorageError> {
        let key = self.message_status_key(id);
        let value = serde_json::to_vec(&status)?;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGE_STATUS_TABLE)?;
            table.insert(key.as_slice(), value.as_slice())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub fn list_all_messages_with_status(
        &self,
    ) -> Result<Vec<(MessageData, MessageStatus)>, StorageError> {
        self.list_messages_with_status_filter(None)
    }

    fn list_messages_with_status_filter(
        &self,
        filter_status: Option<MessageStatus>,
    ) -> Result<Vec<(MessageData, MessageStatus)>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let messages_table = read_txn.open_table(MESSAGES_TABLE)?;
        let status_table = read_txn.open_table(MESSAGE_STATUS_TABLE)?;

        let mut results = Vec::new();
        let msg_prefix = self.prefix_only(b"msg:");

        for result in messages_table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();

            if key_bytes.starts_with(&msg_prefix) && key_bytes.len() == msg_prefix.len() + 32 {
                let msg_id = B256::from_slice(&key_bytes[msg_prefix.len()..]);
                let status_key = self.message_status_key(&msg_id);
                let status = match status_table.get(status_key.as_slice())? {
                    Some(v) => serde_json::from_slice(v.value())?,
                    None => MessageStatus::Pending,
                };

                if filter_status.is_none() || filter_status == Some(status) {
                    let msg: MessageData = serde_json::from_slice(value.value())?;
                    results.push((msg, status));
                }
            }
        }

        Ok(results)
    }

    /// Read-modify-write a provider artifact inside a single write transaction.
    ///
    /// The closure receives the current artifact (or `None` if missing) and
    /// returns the replacement (or `None` to skip writing). All callers that
    /// need to mutate an existing `ProviderArtifact` should go through here
    /// so the read and write share a transaction — otherwise concurrent
    /// writers can clobber each other's updates (see issue #64).
    fn update_provider_artifact<F>(&self, artifact_id: &str, f: F) -> Result<(), StorageError>
    where
        F: FnOnce(Option<ProviderArtifact>) -> Result<Option<ProviderArtifact>, StorageError>,
    {
        let artifact_key = self.artifact_key(artifact_id);

        let write_txn = self.db.begin_write()?;
        {
            let mut artifacts_table = write_txn.open_table(PROVIDER_ARTIFACTS_TABLE)?;

            let existing: Option<ProviderArtifact> =
                match artifacts_table.get(artifact_key.as_slice())? {
                    Some(v) => Some(serde_json::from_slice(v.value())?),
                    None => None,
                };

            if let Some(artifact) = f(existing)? {
                let artifact_value = serde_json::to_vec(&artifact)?;
                artifacts_table.insert(artifact_key.as_slice(), artifact_value.as_slice())?;

                let mut map_table = write_txn.open_table(ARTIFACT_BY_MESSAGE_TABLE)?;
                for msg_id in &artifact.message_ids {
                    if *msg_id != B256::ZERO {
                        let key = self.artifact_by_message_key(msg_id);
                        map_table.insert(key.as_slice(), artifact.artifact_id.as_bytes())?;
                    }
                }
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    pub fn get_provider_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<Option<ProviderArtifact>, StorageError> {
        let key = self.artifact_key(artifact_id);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PROVIDER_ARTIFACTS_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

    pub fn map_message_to_artifact(
        &self,
        message_id: &B256,
        artifact_id: &str,
    ) -> Result<(), StorageError> {
        let key = self.artifact_by_message_key(message_id);

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(ARTIFACT_BY_MESSAGE_TABLE)?;
            table.insert(key.as_slice(), artifact_id.as_bytes())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub fn get_artifact_for_message(
        &self,
        message_id: &B256,
    ) -> Result<Option<String>, StorageError> {
        let key = self.artifact_by_message_key(message_id);

        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(ARTIFACT_BY_MESSAGE_TABLE)?;

        Ok(table
            .get(key.as_slice())?
            .map(|v| String::from_utf8_lossy(v.value()).to_string()))
    }

    // Merkle compatibility methods backed by provider artifacts
    pub fn save_merkle_tree(&self, tree: &MerkleTreeData) -> Result<(), StorageError> {
        let artifact_id = tree.root_hash.to_string();
        self.update_provider_artifact(&artifact_id, |existing| {
            let pending_request_id = if tree.proof.is_empty() {
                existing.as_ref().and_then(|a| a.pending_request_id.clone())
            } else {
                None
            };

            let mut artifact = ProviderArtifact::new_merkle(tree, pending_request_id)?;
            if let Some(existing) = existing {
                artifact.created_at = existing.created_at;
                artifact.updated_at = unix_timestamp();
            }
            Ok(Some(artifact))
        })
    }

    pub fn get_merkle_tree_by_root(
        &self,
        root: &B256,
    ) -> Result<Option<MerkleTreeData>, StorageError> {
        let artifact_id = root.to_string();
        match self.get_provider_artifact(&artifact_id)? {
            Some(artifact) if artifact.is_merkle_kind() => Ok(Some(artifact.as_merkle_tree()?)),
            Some(_) => Ok(None),
            None => Ok(None),
        }
    }

    pub fn get_merkle_root_by_message(
        &self,
        message_id: &B256,
    ) -> Result<Option<B256>, StorageError> {
        let artifact_id = match self.get_artifact_for_message(message_id)? {
            Some(id) => id,
            None => return Ok(None),
        };

        if let Ok(root) = artifact_id.parse::<B256>() {
            return Ok(Some(root));
        }

        Ok(self
            .get_provider_artifact(&artifact_id)?
            .and_then(|a| a.root_hash))
    }

    pub fn list_pending_merkle_roots(&self) -> Result<HashMap<B256, Option<String>>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PROVIDER_ARTIFACTS_TABLE)?;

        let mut roots = HashMap::new();
        let prefix = self.prefix_only(b"artifact:");

        for result in table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();
            if !key_bytes.starts_with(&prefix) {
                continue;
            }

            let artifact: ProviderArtifact = serde_json::from_slice(value.value())?;
            if !artifact.is_merkle_kind() {
                continue;
            }

            let tree = artifact.as_merkle_tree()?;
            if tree.proof.is_empty() {
                roots.insert(tree.root_hash, artifact.pending_request_id.clone());
            }
        }

        Ok(roots)
    }

    pub fn get_pending_request_id(&self, root: &B256) -> Result<Option<String>, StorageError> {
        let artifact_id = root.to_string();
        Ok(self
            .get_provider_artifact(&artifact_id)?
            .and_then(|a| a.pending_request_id))
    }

    pub fn set_pending_request_id(
        &self,
        root: &B256,
        request_id: &str,
    ) -> Result<(), StorageError> {
        let artifact_id = root.to_string();
        self.update_provider_artifact(&artifact_id, |existing| {
            let mut artifact = existing.ok_or_else(|| {
                StorageError::NotFound(format!("artifact not found: {artifact_id}"))
            })?;
            artifact.pending_request_id = Some(request_id.to_string());
            artifact.updated_at = unix_timestamp();
            Ok(Some(artifact))
        })
    }

    pub fn delete_pending(&self, root: &B256) -> Result<(), StorageError> {
        let artifact_id = root.to_string();
        self.update_provider_artifact(&artifact_id, |existing| {
            let Some(mut artifact) = existing else {
                return Ok(None);
            };
            artifact.pending_request_id = None;
            artifact.updated_at = unix_timestamp();
            Ok(Some(artifact))
        })
    }

    pub fn save_submission_status(&self, status: &SubmissionStatus) -> Result<(), StorageError> {
        let key = self.submission_status_key(status.destination_chain, &status.message_id);

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(SUBMISSION_STATUS_TABLE)?;
            let mut to_save = status.clone();
            if let Some(existing) = table.get(key.as_slice())? {
                let existing_status: SubmissionStatus = serde_json::from_slice(existing.value())?;
                to_save.created_at = existing_status.created_at;
            }

            let value = serde_json::to_vec(&to_save)?;
            table.insert(key.as_slice(), value.as_slice())?;

            if let Some(ref idem_key) = status.idempotency_key {
                let mut idem_table = write_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;
                let idem_index_key = self.idempotency_index_key(idem_key);
                idem_table.insert(idem_index_key.as_slice(), key.as_slice())?;
            }

            if let Some(ref tx_id) = status.relayer_tx_id {
                let mut tx_table = write_txn.open_table(RELAYER_TX_INDEX_TABLE)?;
                let tx_index_key = self.relayer_tx_index_key(tx_id);
                tx_table.insert(tx_index_key.as_slice(), key.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_submission_status(
        &self,
        chain_id: u64,
        message_id: &B256,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let key = self.submission_status_key(chain_id, message_id);
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        table
            .get(key.as_slice())?
            .map(|v| serde_json::from_slice(v.value()))
            .transpose()
            .map_err(Into::into)
    }

    pub fn list_signed_trees_without_submissions(
        &self,
    ) -> Result<Vec<MerkleTreeData>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let artifacts_table = read_txn.open_table(PROVIDER_ARTIFACTS_TABLE)?;
        let submissions_table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        let mut trees = Vec::new();
        let prefix = self.prefix_only(b"artifact:");

        for result in artifacts_table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();
            if !key_bytes.starts_with(&prefix) {
                continue;
            }

            let artifact: ProviderArtifact = serde_json::from_slice(value.value())?;
            if !artifact.is_merkle_kind() {
                continue;
            }

            let tree = artifact.as_merkle_tree()?;
            if tree.proof.is_empty() {
                continue;
            }

            let mut needs_submission = false;
            for msg_id in &tree.message_ids {
                let sub_key = self.submission_status_key(tree.destination_chain, msg_id);
                match submissions_table.get(sub_key.as_slice())? {
                    Some(v) => {
                        let status: SubmissionStatus = serde_json::from_slice(v.value())?;
                        // Failed counts as terminal: the submitter skips any
                        // non-Pending row, so leaving Failed as "needs
                        // submission" spins forever. Retrying requires an
                        // explicit row reset.
                        if !matches!(
                            status.status,
                            SubmissionState::Confirmed
                                | SubmissionState::Deduplicated
                                | SubmissionState::Failed
                        ) {
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

        Ok(trees)
    }

    pub fn get_submission_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let idem_table = read_txn.open_table(IDEMPOTENCY_INDEX_TABLE)?;
        let index_key = self.idempotency_index_key(key);

        match idem_table.get(index_key.as_slice())? {
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

    pub fn get_submission_by_relayer_tx_id(
        &self,
        tx_id: &str,
    ) -> Result<Option<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let tx_table = read_txn.open_table(RELAYER_TX_INDEX_TABLE)?;
        let index_key = self.relayer_tx_index_key(tx_id);

        match tx_table.get(index_key.as_slice())? {
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

    pub fn list_pending_relayer_submissions(&self) -> Result<Vec<SubmissionStatus>, StorageError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SUBMISSION_STATUS_TABLE)?;

        let mut submissions = Vec::new();
        let prefix = self.prefix_only(b"submission:");

        for result in table.iter()? {
            let (key, value) = result?;
            let key_bytes = key.value();

            if key_bytes.starts_with(&prefix) {
                let status: SubmissionStatus = serde_json::from_slice(value.value())?;
                if status.relayer_tx_id.is_some()
                    && !matches!(
                        status.status,
                        SubmissionState::Confirmed
                            | SubmissionState::Failed
                            | SubmissionState::Deduplicated
                    )
                {
                    submissions.push(status);
                }
            }
        }

        Ok(submissions)
    }

    #[inline]
    fn prefix_with_provider(&self, prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
        let mut key = Vec::with_capacity(prefix.len() + self.provider.len() + 1 + suffix.len());
        key.extend_from_slice(prefix);
        key.extend_from_slice(self.provider.as_bytes());
        key.push(b':');
        key.extend_from_slice(suffix);
        key
    }

    #[inline]
    fn prefix_only(&self, prefix: &[u8]) -> Vec<u8> {
        self.prefix_with_provider(prefix, &[])
    }

    fn message_key(&self, id: &B256) -> Vec<u8> {
        self.prefix_with_provider(b"msg:", id.as_slice())
    }

    fn message_status_key(&self, id: &B256) -> Vec<u8> {
        self.prefix_with_provider(b"msgstatus:", id.as_slice())
    }

    fn artifact_key(&self, artifact_id: &str) -> Vec<u8> {
        self.prefix_with_provider(b"artifact:", artifact_id.as_bytes())
    }

    fn artifact_by_message_key(&self, id: &B256) -> Vec<u8> {
        self.prefix_with_provider(b"artmsg:", id.as_slice())
    }

    fn submission_status_key(&self, chain_id: u64, message_id: &B256) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(8 + 1 + 32);
        suffix.extend_from_slice(&chain_id.to_be_bytes());
        suffix.push(b':');
        suffix.extend_from_slice(message_id.as_slice());
        self.prefix_with_provider(b"submission:", &suffix)
    }

    fn idempotency_index_key(&self, idempotency_key: &str) -> Vec<u8> {
        self.prefix_with_provider(b"idem:", idempotency_key.as_bytes())
    }

    fn relayer_tx_index_key(&self, relayer_tx_id: &str) -> Vec<u8> {
        self.prefix_with_provider(b"reltx:", relayer_tx_id.as_bytes())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

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
    fn test_provider_scoped_message_isolation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let msg_id = B256::from_slice(&[0x11u8; 32]);

        let storage_a = Storage::new_with_provider(&path, "layerzero").unwrap();
        storage_a
            .save_message(&test_message(msg_id, 1, 31338, 100))
            .unwrap();
        assert!(storage_a.get_message(&msg_id).unwrap().is_some());
        drop(storage_a);

        let storage_b = Storage::new_with_provider(&path, "chainlink_ccv").unwrap();
        assert!(storage_b.get_message(&msg_id).unwrap().is_none());
        drop(storage_b);

        let storage_a = Storage::new_with_provider(&path, "layerzero").unwrap();
        assert!(storage_a.get_message(&msg_id).unwrap().is_some());
    }

    #[test]
    fn test_save_and_get_merkle_via_artifact() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new_with_provider(&path, "layerzero").unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xBBu8; 32])],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![],
            epoch: None,
        };

        storage.save_merkle_tree(&tree).unwrap();
        let found = storage
            .get_merkle_tree_by_root(&tree.root_hash)
            .unwrap()
            .unwrap();
        assert_eq!(found.root_hash, tree.root_hash);

        let pending = storage.list_pending_merkle_roots().unwrap();
        assert!(pending.contains_key(&tree.root_hash));

        storage
            .set_pending_request_id(&tree.root_hash, "req-123")
            .unwrap();
        let req = storage.get_pending_request_id(&tree.root_hash).unwrap();
        assert_eq!(req.as_deref(), Some("req-123"));
    }

    #[test]
    fn test_submission_status_provider_scoped_indexes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let msg_id = B256::from_slice(&[0x22u8; 32]);
        let mut status = SubmissionStatus::new_pending_with_key(
            msg_id,
            B256::from_slice(&[0x33u8; 32]),
            31338,
            "bg-layerzero-1122-aabb".to_string(),
        );
        status.set_relayer_tx_id("tx-1".to_string());

        {
            let storage_lz = Storage::new_with_provider(&path, "layerzero").unwrap();
            storage_lz.save_submission_status(&status).unwrap();
            assert!(
                storage_lz
                    .get_submission_by_idempotency_key("bg-layerzero-1122-aabb")
                    .unwrap()
                    .is_some()
            );
            assert!(
                storage_lz
                    .get_submission_by_relayer_tx_id("tx-1")
                    .unwrap()
                    .is_some()
            );
        }

        {
            let storage_ccv = Storage::new_with_provider(&path, "chainlink_ccv").unwrap();
            assert!(
                storage_ccv
                    .get_submission_by_idempotency_key("bg-layerzero-1122-aabb")
                    .unwrap()
                    .is_none()
            );
            assert!(
                storage_ccv
                    .get_submission_by_relayer_tx_id("tx-1")
                    .unwrap()
                    .is_none()
            );
        }

        {
            let storage_lz = Storage::new_with_provider(&path, "layerzero").unwrap();
            assert!(
                storage_lz
                    .get_submission_by_idempotency_key("bg-layerzero-1122-aabb")
                    .unwrap()
                    .is_some()
            );
            assert!(
                storage_lz
                    .get_submission_by_relayer_tx_id("tx-1")
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn test_new_with_empty_provider_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let result = Storage::new_with_provider(&path, "");
        assert!(result.is_err());
    }

    #[test]
    fn test_new_with_whitespace_provider_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let result = Storage::new_with_provider(&path, "   ");
        assert!(result.is_err());
    }

    #[test]
    fn test_duplicate_message_ignored() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let msg = test_message(msg_id, 1, 31338, 100);

        // First save succeeds
        storage.save_message(&msg).unwrap();

        // Second save is silently ignored (idempotent)
        storage.save_message(&msg).unwrap();

        // Only one message present
        let all = storage.list_all_messages_with_status().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_get_message_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let result = storage.get_message(&B256::ZERO).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update_and_list_message_status() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        storage
            .save_message(&test_message(msg_id, 1, 31338, 100))
            .unwrap();

        // Starts as Pending
        let pending = storage
            .list_messages_by_status(MessageStatus::Pending)
            .unwrap();
        assert_eq!(pending.len(), 1);

        // Update to Signed
        storage
            .update_message_status(&msg_id, MessageStatus::Signed)
            .unwrap();

        let pending = storage
            .list_messages_by_status(MessageStatus::Pending)
            .unwrap();
        assert!(pending.is_empty());

        let signed = storage
            .list_messages_by_status(MessageStatus::Signed)
            .unwrap();
        assert_eq!(signed.len(), 1);
    }

    #[test]
    fn test_list_pending_relayer_submissions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Pending without relayer_tx_id should NOT appear
        let msg1 = B256::from_slice(&[0x01u8; 32]);
        let status1 = SubmissionStatus::new_pending(msg1, B256::ZERO, 31338);
        storage.save_submission_status(&status1).unwrap();

        // Submitted with relayer_tx_id should appear
        let msg2 = B256::from_slice(&[0x02u8; 32]);
        let mut status2 = SubmissionStatus::new_pending(msg2, B256::ZERO, 31338);
        status2.set_relayer_tx_id("tx-1".to_string());
        storage.save_submission_status(&status2).unwrap();

        // Confirmed should NOT appear
        let msg3 = B256::from_slice(&[0x03u8; 32]);
        let mut status3 = SubmissionStatus::new_pending(msg3, B256::ZERO, 31338);
        status3.set_relayer_tx_id("tx-2".to_string());
        status3.mark_confirmed(None);
        storage.save_submission_status(&status3).unwrap();

        let pending = storage.list_pending_relayer_submissions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, msg2);
    }

    #[test]
    fn test_delete_pending_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Should not error on nonexistent root
        storage.delete_pending(&B256::ZERO).unwrap();
    }

    #[test]
    fn test_get_merkle_root_by_message() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root = B256::from_slice(&[0xAAu8; 32]);

        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xBBu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let found_root = storage.get_merkle_root_by_message(&msg_id).unwrap();
        assert_eq!(found_root, Some(root));

        // Unknown message returns None
        let unknown = storage
            .get_merkle_root_by_message(&B256::from_slice(&[0xFFu8; 32]))
            .unwrap();
        assert!(unknown.is_none());
    }

    #[test]
    fn test_provider_accessor() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new_with_provider(&path, "layerzero").unwrap();
        assert_eq!(storage.provider(), "layerzero");
    }

    #[test]
    fn test_provider_normalizes_case() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new_with_provider(&path, "LayerZero").unwrap();
        assert_eq!(storage.provider(), "layerzero");
    }

    #[test]
    fn test_map_message_to_artifact_and_lookup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0xFFu8; 32]);
        storage
            .map_message_to_artifact(&msg_id, "custom-artifact-id")
            .unwrap();

        let artifact_id = storage.get_artifact_for_message(&msg_id).unwrap();
        assert_eq!(artifact_id, Some("custom-artifact-id".to_string()));
    }

    #[test]
    fn test_get_artifact_for_message_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0xEEu8; 32]);
        let result = storage.get_artifact_for_message(&msg_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_signed_trees_without_submissions_returns_signed_trees() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        // Signed tree (has proof)
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xCCu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![0u8; 96], // non-empty = signed
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].root_hash, root);
    }

    #[test]
    fn test_list_signed_trees_without_submissions_skips_confirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let root = B256::from_slice(&[0xBBu8; 32]);

        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xCCu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![0u8; 96],
            epoch: Some(1),
        };
        storage.save_merkle_tree(&tree).unwrap();

        // Mark the message as confirmed
        let mut status = SubmissionStatus::new_pending(msg_id, root, 31338);
        status.mark_confirmed(None);
        storage.save_submission_status(&status).unwrap();

        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert!(trees.is_empty());
    }

    #[test]
    fn test_list_signed_trees_without_submissions_skips_unsigned() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let msg_id = B256::from_slice(&[0x01u8; 32]);

        // Unsigned tree (empty proof)
        let tree = MerkleTreeData {
            root_hash: B256::from_slice(&[0xBBu8; 32]),
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xCCu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        let trees = storage.list_signed_trees_without_submissions().unwrap();
        assert!(trees.is_empty());
    }

    #[test]
    fn test_list_pending_relayer_submissions_skips_no_relayer_tx_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        // Failed status with relayer_tx_id should NOT appear
        let msg = B256::from_slice(&[0x10u8; 32]);
        let mut status = SubmissionStatus::new_pending(msg, B256::ZERO, 31338);
        status.set_relayer_tx_id("tx-fail".to_string());
        status.mark_failed();
        storage.save_submission_status(&status).unwrap();

        let pending = storage.list_pending_relayer_submissions().unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_delete_pending_removes_request_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![],
            leaf_hashes: vec![],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![],
            proof: vec![],
            epoch: None,
        };
        storage.save_merkle_tree(&tree).unwrap();

        storage.set_pending_request_id(&root, "req-abc").unwrap();
        assert!(storage.get_pending_request_id(&root).unwrap().is_some());

        storage.delete_pending(&root).unwrap();
        assert!(storage.get_pending_request_id(&root).unwrap().is_none());
    }

    #[test]
    fn test_set_pending_request_id_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let result = storage.set_pending_request_id(&B256::ZERO, "req-abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_merkle_tree_by_root_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let result = storage.get_merkle_tree_by_root(&B256::ZERO).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_merkle_tree_preserves_pending_request_id_and_created_at() {
        // Regression for issue #64: `save_merkle_tree` previously did
        // read(tx1) → read(tx2) → write(tx3), which lost concurrent updates
        // to `pending_request_id` / `created_at`. The fix reads existing
        // state inside the same write transaction. This test pins the
        // resulting merge semantics so a future refactor cannot silently
        // drop them.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let unsigned_tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xBBu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![],
            epoch: None,
        };

        // Seed: initial save and pending_request_id set by some other worker.
        storage.save_merkle_tree(&unsigned_tree).unwrap();
        let original_created_at = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap()
            .created_at;
        storage.set_pending_request_id(&root, "req-xyz").unwrap();

        // Re-saving the still-unsigned tree must preserve both fields via the
        // in-transaction read/merge.
        storage.save_merkle_tree(&unsigned_tree).unwrap();
        let artifact = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(artifact.created_at, original_created_at);
        assert_eq!(artifact.pending_request_id.as_deref(), Some("req-xyz"));

        // Saving with a non-empty proof keeps created_at but clears
        // pending_request_id (existing semantics: a signed tree no longer has
        // an outstanding signing request).
        let signed_tree = MerkleTreeData {
            proof: vec![0u8; 96],
            ..unsigned_tree
        };
        storage.save_merkle_tree(&signed_tree).unwrap();
        let artifact = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(artifact.created_at, original_created_at);
        assert_eq!(artifact.pending_request_id, None);

        // The message-to-artifact index is still wired up after the re-saves.
        assert_eq!(
            storage.get_artifact_for_message(&msg_id).unwrap(),
            Some(root.to_string())
        );
    }

    #[test]
    fn test_set_pending_request_id_preserves_payload() {
        // `set_pending_request_id` must only touch pending_request_id and
        // updated_at; payload (proof, epoch, message_ids) and created_at
        // must survive intact. Before the refactor, the read+write spanned
        // two transactions, so a concurrent `save_merkle_tree` writing a
        // fresh proof in between could be clobbered. The refactor reads
        // the artifact inside the same write txn — this test pins the
        // structural property.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let signed_tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xBBu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![0u8; 96],
            epoch: Some(7),
        };
        storage.save_merkle_tree(&signed_tree).unwrap();
        let original_created_at = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap()
            .created_at;

        storage.set_pending_request_id(&root, "req-zzz").unwrap();

        let artifact = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(artifact.pending_request_id.as_deref(), Some("req-zzz"));
        assert_eq!(artifact.created_at, original_created_at);

        let tree = artifact.as_merkle_tree().unwrap();
        assert_eq!(tree.proof, vec![0u8; 96]);
        assert_eq!(tree.epoch, Some(7));
        assert_eq!(tree.message_ids, vec![msg_id]);
    }

    #[test]
    fn test_delete_pending_preserves_payload() {
        // Same shape as the set-pending test: `delete_pending` clears
        // pending_request_id without touching payload or created_at.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();

        let root = B256::from_slice(&[0xAAu8; 32]);
        let msg_id = B256::from_slice(&[0x01u8; 32]);
        let signed_tree = MerkleTreeData {
            root_hash: root,
            message_ids: vec![msg_id],
            leaf_hashes: vec![B256::from_slice(&[0xBBu8; 32])],
            source_chain: 1,
            destination_chain: 31338,
            block_numbers: vec![42],
            proof: vec![0u8; 96],
            epoch: Some(7),
        };
        storage.save_merkle_tree(&signed_tree).unwrap();
        storage.set_pending_request_id(&root, "req-zzz").unwrap();
        let original_created_at = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap()
            .created_at;

        storage.delete_pending(&root).unwrap();

        let artifact = storage
            .get_provider_artifact(&root.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(artifact.pending_request_id, None);
        assert_eq!(artifact.created_at, original_created_at);

        let tree = artifact.as_merkle_tree().unwrap();
        assert_eq!(tree.proof, vec![0u8; 96]);
        assert_eq!(tree.epoch, Some(7));
    }

    #[test]
    fn test_save_submission_status_preserves_created_at() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new_with_provider(&path, "layerzero").unwrap();
        let msg_id = B256::from_slice(&[0x44u8; 32]);
        let root = B256::from_slice(&[0x55u8; 32]);

        let mut first = SubmissionStatus::new_pending_with_key(
            msg_id,
            root,
            31338,
            "bg-layerzero-44-55".to_string(),
        );
        first.created_at = 10;
        first.updated_at = 10;
        storage.save_submission_status(&first).unwrap();

        let mut replacement = SubmissionStatus::new_pending_with_key(
            msg_id,
            root,
            31338,
            "bg-layerzero-44-55".to_string(),
        );
        replacement.created_at = 999;
        replacement.updated_at = 999;
        replacement.set_relayer_tx_id("tx-replaced".to_string());
        storage.save_submission_status(&replacement).unwrap();

        let saved = storage
            .get_submission_status(31338, &msg_id)
            .unwrap()
            .unwrap();
        assert_eq!(saved.created_at, 10);
        assert_eq!(saved.relayer_tx_id.as_deref(), Some("tx-replaced"));
    }
}
