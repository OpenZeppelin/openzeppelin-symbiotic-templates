use std::sync::Arc;

use alloy::primitives::{Address, B256, Bytes, keccak256};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use axum::Router;

use super::types::ChainlinkCcvConfig;
use super::{PreparedSubmission, Provider};
use crate::api::AppState;
use crate::config::AppConfig;
use crate::crypto::MerkleProof;
use crate::error::ProviderError;
use crate::evm::{DecodedCcipMessageSent, ccip_message_sent_topic};
use crate::storage::{MerkleTreeData, MessageData, MessageMetadata, Storage};
use crate::webhook::WebhookEvent;

sol! {
    #[derive(Debug)]
    interface IOffRampExecute {
        function execute(
            bytes calldata encodedMessage,
            address[] calldata ccvs,
            bytes[] calldata verifierResults,
            uint32 gasLimitOverride
        ) external;
    }
}

fn encode_offramp_execute(
    encoded_message: &[u8],
    ccvs: Vec<Address>,
    verifier_results: Vec<Vec<u8>>,
    gas_limit_override: u32,
) -> Bytes {
    let verifier_results: Vec<Bytes> = verifier_results.into_iter().map(Bytes::from).collect();

    let call = IOffRampExecute::executeCall {
        encodedMessage: Bytes::copy_from_slice(encoded_message),
        ccvs,
        verifierResults: verifier_results,
        gasLimitOverride: gas_limit_override,
    };

    Bytes::from(call.abi_encode())
}

/// Chainlink CCV provider implementation.
pub struct ChainlinkCcvProvider {
    config: ChainlinkCcvConfig,
    app_config: Arc<AppConfig>,
    storage: Arc<Storage>,
    source_onramp_address: Address,
    destination_ccv_address: Address,
    destination_offramp_address: Address,
}

impl ChainlinkCcvProvider {
    pub fn new(
        config: ChainlinkCcvConfig,
        app_config: Arc<AppConfig>,
        storage: Arc<Storage>,
    ) -> Result<Self, ProviderError> {
        let source_onramp_address = config.source_onramp_address.parse().map_err(|e| {
            ProviderError::EventDecode(format!("invalid source onRamp address: {e}"))
        })?;
        let destination_ccv_address = config.destination_ccv_address.parse().map_err(|e| {
            ProviderError::EventDecode(format!("invalid destination CCV address: {e}"))
        })?;
        let destination_offramp_address =
            config.destination_offramp_address.parse().map_err(|e| {
                ProviderError::EventDecode(format!("invalid destination offRamp address: {e}"))
            })?;

        Ok(Self {
            config,
            app_config,
            storage,
            source_onramp_address,
            destination_ccv_address,
            destination_offramp_address,
        })
    }

    fn valid_event(&self, dest_chain_selector: u64) -> bool {
        dest_chain_selector == self.config.destination_chain_selector
            && self
                .app_config
                .is_supported_destination(self.config.destination_chain_id)
    }

    fn extract_version_tag(verifier_blobs: &[Vec<u8>]) -> Result<[u8; 4], ProviderError> {
        let blob = verifier_blobs
            .first()
            .ok_or_else(|| ProviderError::EventDecode("missing verifier blob".to_string()))?;

        if blob.len() < 4 {
            return Err(ProviderError::EventDecode(
                "verifier blob shorter than 4-byte version tag".to_string(),
            ));
        }

        Ok([blob[0], blob[1], blob[2], blob[3]])
    }

    fn encode_epoch_u48(epoch: u64) -> Result<[u8; 6], ProviderError> {
        if epoch > 0x0000_FFFF_FFFF_FFFF {
            return Err(ProviderError::EventDecode(format!(
                "epoch {epoch} exceeds uint48 range"
            )));
        }

        let be = epoch.to_be_bytes();
        Ok([be[2], be[3], be[4], be[5], be[6], be[7]])
    }

    fn build_settlement_signing_message(version: [u8; 4], message_id: B256) -> Vec<u8> {
        let mut payload = Vec::with_capacity(36);
        payload.extend_from_slice(&version);
        payload.extend_from_slice(message_id.as_slice());
        payload
    }
}

#[async_trait]
impl Provider for ChainlinkCcvProvider {
    fn name(&self) -> &'static str {
        "chainlink_ccv"
    }

    async fn handle_webhook_event(&self, event: &WebhookEvent) -> Result<(), ProviderError> {
        let expected_topic = ccip_message_sent_topic();

        for log in &event.evm.logs {
            if log.topics.is_empty() || log.topics[0] != expected_topic {
                continue;
            }

            if log.address != self.source_onramp_address {
                tracing::debug!(
                    expected_onramp = %self.source_onramp_address,
                    got = %log.address,
                    "ignoring CCIPMessageSent event from unexpected emitter"
                );
                continue;
            }

            let alloy_log = log.to_alloy_log();
            let msg_event = DecodedCcipMessageSent::decode_log(&alloy_log).map_err(|e| {
                ProviderError::EventDecode(format!("failed to decode CCIPMessageSent: {e}"))
            })?;

            if !self.valid_event(msg_event.dest_chain_selector) {
                tracing::debug!(
                    message_id = %msg_event.message_id,
                    dest_chain_selector = msg_event.dest_chain_selector,
                    "ignoring CCIPMessageSent event for unsupported destination"
                );
                continue;
            }

            let source_chain = event
                .evm
                .transaction
                .as_ref()
                .and_then(|tx| tx.chain_id)
                .unwrap_or(self.config.source_chain_id);

            let message = MessageData {
                metadata: MessageMetadata {
                    source_chain,
                    destination_chain: self.config.destination_chain_id,
                    block_number: log.block_number,
                    message_id: msg_event.message_id,
                    event_tx_hash: log.transaction_hash,
                    ttl: None,
                },
                data: serde_json::to_vec(&msg_event)?,
            };

            self.storage.save_message(&message)?;

            tracing::info!(
                message_id = %msg_event.message_id,
                source_chain,
                destination_chain = self.config.destination_chain_id,
                block = log.block_number,
                "stored CCIPMessageSent event"
            );
        }

        Ok(())
    }

    fn register_api_routes(&self, router: Router<AppState>) -> Router<AppState> {
        router
    }

    fn max_batch_size(&self) -> usize {
        1
    }

    fn compute_leaf_hash(&self, message: &MessageData) -> Result<B256, ProviderError> {
        let msg_event: DecodedCcipMessageSent = serde_json::from_slice(&message.data)?;
        let version = Self::extract_version_tag(&msg_event.verifier_blobs)?;
        let payload = Self::build_settlement_signing_message(version, msg_event.message_id);
        Ok(keccak256(payload))
    }

    fn encode_signing_message(&self, tree: &MerkleTreeData) -> Result<Vec<u8>, ProviderError> {
        if tree.message_ids.len() != 1 {
            return Err(ProviderError::EventDecode(format!(
                "chainlink_ccv expects single-message trees, got {} messages",
                tree.message_ids.len()
            )));
        }

        let message_id = tree.message_ids[0];
        let message = self.storage.get_message(&message_id)?.ok_or_else(|| {
            ProviderError::EventDecode(format!(
                "missing message payload for tree message_id {}",
                message_id
            ))
        })?;

        let msg_event: DecodedCcipMessageSent = serde_json::from_slice(&message.data)?;
        let version = Self::extract_version_tag(&msg_event.verifier_blobs)?;
        let signing_message = Self::build_settlement_signing_message(version, message_id);
        let expected_root = keccak256(&signing_message);

        if expected_root != tree.root_hash {
            return Err(ProviderError::EventDecode(format!(
                "tree root mismatch for message_id {}: expected {}, got {}",
                message_id, expected_root, tree.root_hash
            )));
        }

        Ok(signing_message)
    }

    fn prepare_submission(
        &self,
        message: &MessageData,
        tree: &MerkleTreeData,
        _proof: &MerkleProof,
        target_address: &str,
    ) -> Result<PreparedSubmission, ProviderError> {
        let msg_event: DecodedCcipMessageSent = serde_json::from_slice(&message.data)?;
        let version = Self::extract_version_tag(&msg_event.verifier_blobs)?;

        let epoch = tree.epoch.ok_or_else(|| {
            ProviderError::EventDecode("missing epoch on signed tree".to_string())
        })?;
        if tree.proof.is_empty() {
            return Err(ProviderError::EventDecode(
                "missing BLS proof on signed tree".to_string(),
            ));
        }

        let mut verifier_result = Vec::with_capacity(4 + 6 + tree.proof.len());
        verifier_result.extend_from_slice(&version);
        verifier_result.extend_from_slice(&Self::encode_epoch_u48(epoch)?);
        verifier_result.extend_from_slice(&tree.proof);

        let submit_target = if target_address.is_empty() {
            self.destination_offramp_address.to_string()
        } else {
            target_address.to_string()
        };

        if submit_target.is_empty() {
            return Err(ProviderError::EventDecode(
                "missing destination offRamp submit target".to_string(),
            ));
        }

        let calldata = encode_offramp_execute(
            &msg_event.encoded_message,
            vec![self.destination_ccv_address],
            vec![verifier_result],
            0,
        );

        Ok(PreparedSubmission {
            to: submit_target,
            calldata: calldata.to_vec(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_tag() {
        let blobs = vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]];
        let version = ChainlinkCcvProvider::extract_version_tag(&blobs).unwrap();
        assert_eq!(version, [0x1a, 0x75, 0xbd, 0x93]);
    }

    #[test]
    fn test_encode_epoch_u48() {
        assert_eq!(
            ChainlinkCcvProvider::encode_epoch_u48(1).unwrap(),
            [0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            ChainlinkCcvProvider::encode_epoch_u48(0x0102_0304_0506).unwrap(),
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]
        );
    }

    #[test]
    fn test_encode_epoch_u48_rejects_out_of_range() {
        let err = ChainlinkCcvProvider::encode_epoch_u48(0x0001_0000_0000_0000).unwrap_err();
        assert!(err.to_string().contains("exceeds uint48 range"));
    }

    #[test]
    fn test_encode_offramp_execute() {
        let calldata = encode_offramp_execute(
            &[0x01, 0x02, 0x03],
            vec![
                "0x1111111111111111111111111111111111111111"
                    .parse()
                    .unwrap(),
            ],
            vec![vec![0xaa, 0xbb]],
            0,
        );

        assert!(!calldata.is_empty());
        assert!(calldata.len() > 4);
    }

    #[test]
    fn test_build_settlement_signing_message() {
        let version = [0x1a, 0x75, 0xbd, 0x93];
        let message_id = B256::from_slice(
            &hex::decode("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .unwrap(),
        );

        let encoded = ChainlinkCcvProvider::build_settlement_signing_message(version, message_id);
        assert_eq!(encoded.len(), 36);
        assert_eq!(&encoded[..4], &version);
        assert_eq!(&encoded[4..], message_id.as_slice());
    }

    #[test]
    fn test_extract_version_tag_empty_blobs() {
        let err = ChainlinkCcvProvider::extract_version_tag(&[]).unwrap_err();
        assert!(err.to_string().contains("missing verifier blob"));
    }

    #[test]
    fn test_extract_version_tag_short_blob() {
        let err = ChainlinkCcvProvider::extract_version_tag(&[vec![0x01, 0x02]]).unwrap_err();
        assert!(err.to_string().contains("shorter than 4-byte"));
    }

    // ============ Provider integration tests with real Storage ============

    use crate::config::{
        AppConfig, DatabaseConfig, LoggingConfig, OzRelayerConfig, SecurityConfig, ServerConfig,
        SignerConfig, SymbioticRelayConfig,
    };
    use crate::evm::DecodedCcipMessageSent;
    use crate::provider::types::ChainlinkCcvConfig;
    use crate::storage::{MessageData, MessageMetadata, Storage};
    use crate::webhook::{EvmData, MatchedOnArgs, MonitorInfo, WebhookEvent, WebhookLog};
    use alloy::primitives::{Bytes as AlBytes, U256};
    use tempfile::tempdir;

    const SOURCE_ONRAMP: &str = "0x1111111111111111111111111111111111111111";
    const DEST_CCV: &str = "0x2222222222222222222222222222222222222222";
    const DEST_OFFRAMP: &str = "0x3333333333333333333333333333333333333333";

    fn test_ccv_config() -> ChainlinkCcvConfig {
        ChainlinkCcvConfig {
            source_chain_id: 31337,
            destination_chain_id: 31338,
            source_chain_selector: 11111,
            destination_chain_selector: 22222,
            source_ccv_address: "0x4444444444444444444444444444444444444444".to_string(),
            destination_ccv_address: DEST_CCV.to_string(),
            source_onramp_address: SOURCE_ONRAMP.to_string(),
            destination_offramp_address: DEST_OFFRAMP.to_string(),
        }
    }

    fn test_app_config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 3000,
                read_timeout: std::time::Duration::from_secs(30),
                write_timeout: std::time::Duration::from_secs(30),
                idle_timeout: std::time::Duration::from_secs(120),
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
                timeout: std::time::Duration::from_secs(30),
                retry_backoff: std::time::Duration::from_secs(1),
            },
            signer: SignerConfig {
                event_poll_interval: std::time::Duration::from_secs(15),
                sign_job_interval: std::time::Duration::from_secs(1),
                sign_worker_count: 2,
                min_batch_size: 1,
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338],
            provider: "chainlink_ccv".to_string(),
            layerzero: None,
            chainlink_ccv: Some(test_ccv_config()),
        })
    }

    fn test_storage() -> (Arc<Storage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new_with_provider(&path, "chainlink_ccv").unwrap();
        (Arc::new(storage), dir)
    }

    fn test_provider(storage: Arc<Storage>) -> ChainlinkCcvProvider {
        ChainlinkCcvProvider::new(test_ccv_config(), test_app_config(), storage).unwrap()
    }

    /// Build a CCIPMessageSent event encoded into a WebhookLog.
    fn build_ccip_webhook_log(onramp: Address, dest_selector: u64, message_id: B256) -> WebhookLog {
        use alloy::sol_types::SolEvent;

        let event = crate::evm::CCIPMessageSent {
            destChainSelector: dest_selector,
            sender: Address::ZERO,
            messageId: message_id,
            feeToken: Address::ZERO,
            tokenAmountBeforeTokenPoolFees: U256::ZERO,
            encodedMessage: AlBytes::from(vec![0x01, 0x02]),
            receipts: vec![],
            verifierBlobs: vec![AlBytes::from(vec![0x1a, 0x75, 0xbd, 0x93, 0x01])],
        };

        let encoded = event.encode_log_data();
        WebhookLog {
            address: onramp,
            topics: encoded.topics().to_vec(),
            data: AlBytes::from(encoded.data.to_vec()),
            block_number: 100,
            transaction_hash: B256::ZERO,
            log_index: 0,
        }
    }

    fn make_webhook_event(logs: Vec<WebhookLog>) -> WebhookEvent {
        WebhookEvent {
            evm: EvmData {
                logs,
                matched_on_args: MatchedOnArgs { events: vec![] },
                monitor: MonitorInfo {
                    name: "test".to_string(),
                },
                network_slug: "test".to_string(),
                receipt: None,
                transaction: None,
            },
        }
    }

    #[test]
    fn test_valid_event_supported() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);
        assert!(provider.valid_event(22222));
    }

    #[test]
    fn test_valid_event_unsupported_selector() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);
        assert!(!provider.valid_event(99999));
    }

    #[tokio::test]
    async fn test_handle_webhook_event_happy_path() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xAAu8; 32]);
        let onramp: Address = SOURCE_ONRAMP.parse().unwrap();
        let log = build_ccip_webhook_log(onramp, 22222, message_id);
        let event = make_webhook_event(vec![log]);

        provider.handle_webhook_event(&event).await.unwrap();

        let stored = storage.get_message(&message_id).unwrap();
        assert!(stored.is_some());
        let stored = stored.unwrap();
        assert_eq!(stored.metadata.message_id, message_id);
        assert_eq!(stored.metadata.source_chain, 31337);
        assert_eq!(stored.metadata.destination_chain, 31338);
    }

    #[tokio::test]
    async fn test_handle_webhook_event_wrong_topic() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());
        let message_id = B256::from_slice(&[0xDDu8; 32]);

        // Log with a non-matching topic
        let log = WebhookLog {
            address: SOURCE_ONRAMP.parse().unwrap(),
            topics: vec![B256::from_slice(&[0xFFu8; 32])],
            data: AlBytes::from(vec![]),
            block_number: 100,
            transaction_hash: B256::ZERO,
            log_index: 0,
        };

        let event = make_webhook_event(vec![log]);
        provider.handle_webhook_event(&event).await.unwrap();

        let stored = storage.get_message(&message_id).unwrap();
        assert!(stored.is_none());
    }

    #[tokio::test]
    async fn test_handle_webhook_event_wrong_address() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xBBu8; 32]);
        let wrong_address: Address = "0x9999999999999999999999999999999999999999"
            .parse()
            .unwrap();
        let log = build_ccip_webhook_log(wrong_address, 22222, message_id);
        let event = make_webhook_event(vec![log]);

        provider.handle_webhook_event(&event).await.unwrap();

        // Message should NOT be stored since emitter doesn't match onramp
        let stored = storage.get_message(&message_id).unwrap();
        assert!(stored.is_none());
    }

    #[tokio::test]
    async fn test_handle_webhook_event_unsupported_dest() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xCCu8; 32]);
        let onramp: Address = SOURCE_ONRAMP.parse().unwrap();
        // Wrong destination chain selector
        let log = build_ccip_webhook_log(onramp, 99999, message_id);
        let event = make_webhook_event(vec![log]);

        provider.handle_webhook_event(&event).await.unwrap();

        let stored = storage.get_message(&message_id).unwrap();
        assert!(stored.is_none());
    }

    #[test]
    fn test_compute_leaf_hash() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xAAu8; 32]);
        let version = [0x1a, 0x75, 0xbd, 0x93u8];
        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id,
            fee_token: Address::ZERO,
            encoded_message: vec![0x01, 0x02],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };

        let leaf = provider.compute_leaf_hash(&msg).unwrap();

        // Expected: keccak256(version ++ message_id)
        let payload = ChainlinkCcvProvider::build_settlement_signing_message(version, message_id);
        let expected = keccak256(payload);
        assert_eq!(leaf, expected);
    }

    #[test]
    fn test_encode_signing_message() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xAAu8; 32]);
        let version = [0x1a, 0x75, 0xbd, 0x93u8];
        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id,
            fee_token: Address::ZERO,
            encoded_message: vec![0x01, 0x02],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };
        storage.save_message(&msg).unwrap();

        let signing_payload =
            ChainlinkCcvProvider::build_settlement_signing_message(version, message_id);
        let root_hash = keccak256(&signing_payload);

        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![message_id],
            leaf_hashes: vec![root_hash],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: vec![],
            epoch: None,
        };

        let result = provider.encode_signing_message(&tree).unwrap();
        assert_eq!(result, signing_payload);
    }

    #[test]
    fn test_encode_signing_message_rejects_multi_message_tree() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);

        let tree = MerkleTreeData {
            root_hash: B256::ZERO,
            message_ids: vec![
                B256::from_slice(&[0x01u8; 32]),
                B256::from_slice(&[0x02u8; 32]),
            ],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: vec![],
            epoch: None,
        };

        let err = provider.encode_signing_message(&tree).unwrap_err();
        assert!(err.to_string().contains("single-message trees"));
    }

    #[test]
    fn test_prepare_submission() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage.clone());

        let message_id = B256::from_slice(&[0xAAu8; 32]);
        let version = [0x1a, 0x75, 0xbd, 0x93u8];
        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id,
            fee_token: Address::ZERO,
            encoded_message: vec![0x01, 0x02],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };

        let signing_payload =
            ChainlinkCcvProvider::build_settlement_signing_message(version, message_id);
        let root_hash = keccak256(&signing_payload);

        let bls_proof = vec![0xBEu8; 96];
        let tree = MerkleTreeData {
            root_hash,
            message_ids: vec![message_id],
            leaf_hashes: vec![root_hash],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: bls_proof.clone(),
            epoch: Some(42),
        };

        let dummy_proof = crate::crypto::MerkleProof {
            leaf: root_hash,
            siblings: vec![],
            path: 0,
        };

        let result = provider
            .prepare_submission(&msg, &tree, &dummy_proof, "")
            .unwrap();
        assert_eq!(result.to, DEST_OFFRAMP);
        assert!(!result.calldata.is_empty());
        // Calldata should start with the execute function selector (4 bytes)
        assert!(result.calldata.len() > 4);
    }

    #[test]
    fn test_prepare_submission_missing_epoch() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);

        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id: B256::ZERO,
            fee_token: Address::ZERO,
            encoded_message: vec![],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id: B256::ZERO,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };

        let tree = MerkleTreeData {
            root_hash: B256::ZERO,
            message_ids: vec![B256::ZERO],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: vec![0xBE; 96],
            epoch: None, // missing
        };

        let dummy_proof = crate::crypto::MerkleProof {
            leaf: B256::ZERO,
            siblings: vec![],
            path: 0,
        };

        let err = provider
            .prepare_submission(&msg, &tree, &dummy_proof, "")
            .unwrap_err();
        assert!(err.to_string().contains("missing epoch"));
    }

    #[test]
    fn test_prepare_submission_missing_proof() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);

        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id: B256::ZERO,
            fee_token: Address::ZERO,
            encoded_message: vec![],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id: B256::ZERO,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };

        let tree = MerkleTreeData {
            root_hash: B256::ZERO,
            message_ids: vec![B256::ZERO],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: vec![], // empty
            epoch: Some(42),
        };

        let dummy_proof = crate::crypto::MerkleProof {
            leaf: B256::ZERO,
            siblings: vec![],
            path: 0,
        };

        let err = provider
            .prepare_submission(&msg, &tree, &dummy_proof, "")
            .unwrap_err();
        assert!(err.to_string().contains("missing BLS proof"));
    }

    #[test]
    fn test_prepare_submission_custom_target() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);

        let msg_event = DecodedCcipMessageSent {
            dest_chain_selector: 22222,
            sender: Address::ZERO,
            message_id: B256::ZERO,
            fee_token: Address::ZERO,
            encoded_message: vec![0x01],
            verifier_blobs: vec![vec![0x1a, 0x75, 0xbd, 0x93, 0x01]],
        };

        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 31337,
                destination_chain: 31338,
                block_number: 100,
                message_id: B256::ZERO,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: serde_json::to_vec(&msg_event).unwrap(),
        };

        let tree = MerkleTreeData {
            root_hash: B256::ZERO,
            message_ids: vec![B256::ZERO],
            leaf_hashes: vec![],
            source_chain: 31337,
            destination_chain: 31338,
            block_numbers: vec![100],
            proof: vec![0xBE; 96],
            epoch: Some(1),
        };

        let dummy_proof = crate::crypto::MerkleProof {
            leaf: B256::ZERO,
            siblings: vec![],
            path: 0,
        };

        let custom = "0x5555555555555555555555555555555555555555";
        let result = provider
            .prepare_submission(&msg, &tree, &dummy_proof, custom)
            .unwrap();
        assert_eq!(result.to, custom);
    }

    #[test]
    fn test_max_batch_size_is_one() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);
        assert_eq!(provider.max_batch_size(), 1);
    }

    #[test]
    fn test_provider_name() {
        let (storage, _dir) = test_storage();
        let provider = test_provider(storage);
        assert_eq!(provider.name(), "chainlink_ccv");
    }
}
