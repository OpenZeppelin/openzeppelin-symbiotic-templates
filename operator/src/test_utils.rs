//! Shared test utilities and fixtures
//!
//! This module provides reusable test helpers to reduce duplication across test modules.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, B256};
use tempfile::TempDir;

use crate::config::{
    AppConfig, ChainRelayerEntry, DatabaseConfig, LayerZeroConfig, LoggingConfig, OzRelayerConfig,
    SecurityConfig, ServerConfig, SignerConfig, SymbioticRelayConfig,
};
use crate::evm::DecodedJobAssigned;
use crate::storage::{MerkleTreeData, MessageData, MessageMetadata, Storage};
use crate::symbiotic_relay::{MockSymbioticRelayClient, SymbioticRelayClientEnum};
use crate::webhook::{
    EvmData, MatchedOnArgs, MonitorInfo, ParsedEvent, WebhookEvent, WebhookLog,
};

/// Create a test storage instance with a temporary database
pub fn test_storage() -> (Storage, TempDir) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("test.db");
    let storage = Storage::new(&path).expect("failed to create test storage");
    (storage, dir)
}

/// Create a test storage instance wrapped in Arc
pub fn test_storage_arc() -> (Arc<Storage>, TempDir) {
    let (storage, dir) = test_storage();
    (Arc::new(storage), dir)
}

/// Create a valid test configuration with all required fields
pub fn test_config() -> AppConfig {
    AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 3000,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(120),
            security: SecurityConfig {
                webhook_secret: Some("a]".repeat(32)),
                oz_relayer_webhook_secret: Some("a]".repeat(32)),
                timestamp_window: Duration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: true,
            },
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
        },
        oz_relayer: OzRelayerConfig {
            base_url: "http://localhost:8080".to_string(),
            poll_interval: Duration::from_secs(5),
            status_poll_interval: Duration::from_secs(30),
            default_speed: "fast".to_string(),
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_backoff: Duration::from_secs(1),
            chain_relayers: vec![ChainRelayerEntry {
                chain_id: 31338,
                relayer_id: "test-relayer-1".to_string(),
                dvn_address: "0x1234567890123456789012345678901234567890".to_string(),
            }],
        },
        destination_chains: vec![31338, 42161],
        provider: "layerzero".to_string(),
        layerzero: Some(LayerZeroConfig {
            eid_to_chain_id: {
                let mut map = HashMap::new();
                map.insert(30101, 1);
                map.insert(30110, 42161);
                map.insert(40231, 31337);
                map.insert(40232, 31338);
                map
            },
            dvn_addresses: {
                let mut map = HashMap::new();
                map.insert(31338, "0x1234567890123456789012345678901234567890".to_string());
                map.insert(42161, "0xabcdef0123456789abcdef0123456789abcdef01".to_string());
                map
            },
        }),
    }
}

/// Create a test configuration wrapped in Arc
pub fn test_config_arc() -> Arc<AppConfig> {
    Arc::new(test_config())
}

/// Create a mock symbiotic relay client for testing
pub fn mock_symbiotic_client() -> SymbioticRelayClientEnum {
    SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new())
}

/// Create a mock webhook event with a JobAssigned log
pub fn mock_webhook_event(chain_id: u64, block_number: u64) -> WebhookEvent {
    WebhookEvent {
        evm: EvmData {
            logs: vec![mock_webhook_log(block_number)],
            matched_on_args: MatchedOnArgs {
                events: vec![ParsedEvent {
                    args: vec![],
                    hex_signature: "0x1234".to_string(),
                    signature: "JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)".to_string(),
                }],
            },
            monitor: MonitorInfo {
                name: "Test Monitor".to_string(),
            },
            network_slug: "test-network".to_string(),
            receipt: None,
            transaction: Some(crate::webhook::TransactionWithMetadata {
                block_hash: B256::ZERO,
                block_number,
                transaction_index: 0,
                from: Address::ZERO,
                to: None,
                hash: B256::from_slice(&[0x02u8; 32]),
                chain_id: Some(chain_id),
            }),
        },
    }
}

/// Create a mock webhook log entry
pub fn mock_webhook_log(block_number: u64) -> WebhookLog {
    WebhookLog {
        address: Address::ZERO,
        topics: vec![crate::evm::job_assigned_topic()],
        data: Bytes::new(),
        block_number,
        transaction_hash: B256::from_slice(&[0x02u8; 32]),
        log_index: 0,
    }
}

/// Create a test message with specified parameters
pub fn test_message(
    message_id: B256,
    source_chain: u64,
    destination_chain: u64,
    block_number: u64,
) -> MessageData {
    MessageData {
        metadata: MessageMetadata {
            source_chain,
            destination_chain,
            block_number,
            message_id,
            event_tx_hash: B256::from_slice(&[0x02u8; 32]),
            ttl: None,
        },
        data: b"test data".to_vec(),
    }
}

/// Create a test message with default chains (1 -> 31338)
pub fn test_message_default(message_id: B256) -> MessageData {
    test_message(message_id, 1, 31338, 12345)
}

/// Create a test merkle tree
pub fn test_merkle_tree(
    root_hash: B256,
    message_ids: Vec<B256>,
    source_chain: u64,
    destination_chain: u64,
) -> MerkleTreeData {
    let leaf_hashes = message_ids
        .iter()
        .map(|id| B256::from_slice(&alloy::primitives::keccak256(id.as_slice()).0))
        .collect();

    MerkleTreeData {
        root_hash,
        message_ids,
        leaf_hashes,
        source_chain,
        destination_chain,
        block_numbers: vec![12345],
        proof: vec![],
        epoch: None,
    }
}

/// Create a signed merkle tree (with proof)
pub fn test_signed_merkle_tree(
    root_hash: B256,
    message_ids: Vec<B256>,
    source_chain: u64,
    destination_chain: u64,
    epoch: u64,
) -> MerkleTreeData {
    let mut tree = test_merkle_tree(root_hash, message_ids, source_chain, destination_chain);
    tree.proof = vec![0u8; 96]; // Mock BLS signature
    tree.epoch = Some(epoch);
    tree
}

/// Create a test DecodedJobAssigned event
pub fn test_decoded_job_assigned(guid: B256, src_eid: u32, dst_eid: u32) -> DecodedJobAssigned {
    DecodedJobAssigned {
        guid,
        src_eid,
        dst_eid,
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

/// Generate a unique B256 from a seed value
pub fn b256_from_seed(seed: u8) -> B256 {
    B256::from_slice(&[seed; 32])
}

/// Generate a sequential B256 (for unique test IDs)
pub fn sequential_b256(n: u64) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[24..32].copy_from_slice(&n.to_be_bytes());
    B256::from_slice(&bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_creation() {
        let (storage, _dir) = test_storage();
        // Should be able to save and retrieve a message
        let msg = test_message_default(b256_from_seed(1));
        storage.save_message(&msg).unwrap();
        let retrieved = storage.get_message(&msg.metadata.message_id).unwrap();
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_config_valid() {
        let config = test_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sequential_b256_unique() {
        let a = sequential_b256(1);
        let b = sequential_b256(2);
        assert_ne!(a, b);
    }

    #[test]
    fn test_helpers_and_fixtures() {
        let msg_id = b256_from_seed(7);
        let msg = test_message_default(msg_id);
        assert_eq!(msg.metadata.message_id, msg_id);

        let ev = mock_webhook_event(31337, 123);
        assert_eq!(ev.evm.logs.len(), 1);
        assert_eq!(ev.evm.monitor.name, "Test Monitor");

        let log = mock_webhook_log(456);
        assert_eq!(log.block_number, 456);

        let tree = test_merkle_tree(B256::from_slice(&[0xAAu8; 32]), vec![msg_id], 1, 31338);
        assert_eq!(tree.destination_chain, 31338);

        let signed = test_signed_merkle_tree(B256::from_slice(&[0xBBu8; 32]), vec![msg_id], 1, 31338, 1);
        assert!(signed.epoch.is_some());

        let job = test_decoded_job_assigned(B256::ZERO, 40231, 40232);
        assert_eq!(job.src_eid, 40231);
    }
}
