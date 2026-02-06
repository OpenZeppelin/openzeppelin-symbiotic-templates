use std::sync::Arc;

use alloy::primitives::{keccak256, Address, B256, Bytes};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use axum::Router;

use super::{PreparedSubmission, Provider};
use super::types::ChainlinkCcvConfig;
use crate::api::AppState;
use crate::config::AppConfig;
use crate::crypto::MerkleProof;
use crate::error::ProviderError;
use crate::evm::{ccip_message_sent_topic, DecodedCcipMessageSent};
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
}

impl ChainlinkCcvProvider {
    pub fn new(config: ChainlinkCcvConfig, app_config: Arc<AppConfig>, storage: Arc<Storage>) -> Self {
        Self {
            config,
            app_config,
            storage,
        }
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

    fn encode_epoch_u48(epoch: u64) -> [u8; 6] {
        let be = epoch.to_be_bytes();
        [be[2], be[3], be[4], be[5], be[6], be[7]]
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
        let message = self
            .storage
            .get_message(&message_id)?
            .ok_or_else(|| {
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

        let epoch = tree
            .epoch
            .ok_or_else(|| ProviderError::EventDecode("missing epoch on signed tree".to_string()))?;
        if tree.proof.is_empty() {
            return Err(ProviderError::EventDecode(
                "missing BLS proof on signed tree".to_string(),
            ));
        }

        let mut verifier_result = Vec::with_capacity(4 + 6 + tree.proof.len());
        verifier_result.extend_from_slice(&version);
        verifier_result.extend_from_slice(&Self::encode_epoch_u48(epoch));
        verifier_result.extend_from_slice(&tree.proof);

        let ccv_addr: Address = self
            .config
            .destination_ccv_address
            .parse()
            .map_err(|e| ProviderError::EventDecode(format!("invalid destination CCV address: {e}")))?;

        let submit_target = if target_address.is_empty() {
            self.config.destination_offramp_address.clone()
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
            vec![ccv_addr],
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
            ChainlinkCcvProvider::encode_epoch_u48(1),
            [0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            ChainlinkCcvProvider::encode_epoch_u48(0x0102_0304_0506),
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]
        );
    }

    #[test]
    fn test_encode_offramp_execute() {
        let calldata = encode_offramp_execute(
            &[0x01, 0x02, 0x03],
            vec!["0x1111111111111111111111111111111111111111".parse().unwrap()],
            vec![vec![0xaa, 0xbb]],
            0,
        );

        assert!(!calldata.is_empty());
        assert!(calldata.len() > 4);
    }

    #[test]
    fn test_build_settlement_signing_message() {
        let version = [0x1a, 0x75, 0xbd, 0x93];
        let message_id =
            B256::from_slice(&hex::decode("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap());

        let encoded = ChainlinkCcvProvider::build_settlement_signing_message(version, message_id);
        assert_eq!(encoded.len(), 36);
        assert_eq!(&encoded[..4], &version);
        assert_eq!(&encoded[4..], message_id.as_slice());
    }
}
