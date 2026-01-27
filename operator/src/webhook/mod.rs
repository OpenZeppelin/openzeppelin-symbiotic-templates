use alloy::primitives::{Address, Bytes, B256};
use serde::{de, Deserialize, Deserializer, Serialize};

/// Deserialize a u64 from either a number or a hex string (0x...)
fn deserialize_u64_or_hex<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Visitor;

    struct U64OrHexVisitor;

    impl<'de> Visitor<'de> for U64OrHexVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u64 integer or hex string")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if let Some(hex_str) = value.strip_prefix("0x") {
                u64::from_str_radix(hex_str, 16).map_err(de::Error::custom)
            } else {
                value.parse::<u64>().map_err(de::Error::custom)
            }
        }
    }

    deserializer.deserialize_any(U64OrHexVisitor)
}

/// Deserialize an optional u64 from either a number or a hex string (0x...)
fn deserialize_option_u64_or_hex<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Visitor;

    struct OptionU64OrHexVisitor;

    impl<'de> Visitor<'de> for OptionU64OrHexVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u64 integer, hex string, or null")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if let Some(hex_str) = value.strip_prefix("0x") {
                u64::from_str_radix(hex_str, 16)
                    .map(Some)
                    .map_err(de::Error::custom)
            } else {
                value.parse::<u64>().map(Some).map_err(de::Error::custom)
            }
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_u64_or_hex(deserializer).map(Some)
        }
    }

    deserializer.deserialize_any(OptionU64OrHexVisitor)
}

/// Top-level webhook event from OZ Monitor (raw payload mode)
/// With payload_mode: "raw", OZ Monitor sends MonitorMatch directly as JSON
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookEvent {
    #[serde(rename = "EVM")]
    pub evm: EvmData,
}

/// EVM event data
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct EvmData {
    pub logs: Vec<WebhookLog>,
    pub matched_on_args: MatchedOnArgs,
    pub monitor: MonitorInfo,
    pub network_slug: String,
    pub receipt: Option<TransactionReceipt>,
    pub transaction: Option<TransactionWithMetadata>,
}

/// Log entry from webhook
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
    #[serde(rename = "blockNumber", deserialize_with = "deserialize_u64_or_hex")]
    pub block_number: u64,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: B256,
    #[serde(rename = "logIndex", deserialize_with = "deserialize_u64_or_hex")]
    pub log_index: u64,
}

impl WebhookLog {
    /// Convert to alloy Log type for decoding
    pub fn to_alloy_log(&self) -> alloy::rpc::types::Log {
        alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: self.address,
                data: alloy::primitives::LogData::new_unchecked(
                    self.topics.clone(),
                    self.data.clone(),
                ),
            },
            block_hash: None,
            block_number: Some(self.block_number),
            block_timestamp: None,
            transaction_hash: Some(self.transaction_hash),
            transaction_index: None,
            log_index: Some(self.log_index),
            removed: false,
        }
    }
}

/// Matched on args data
#[derive(Debug, Clone, Deserialize)]
pub struct MatchedOnArgs {
    pub events: Vec<ParsedEvent>,
}

/// Parsed event data
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ParsedEvent {
    pub args: Vec<EventArg>,
    pub hex_signature: String,
    pub signature: String,
}

/// Event argument
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct EventArg {
    pub indexed: bool,
    pub kind: String,
    pub name: String,
    pub value: serde_json::Value,
}

/// Monitor info
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorInfo {
    pub name: String,
}

/// Transaction receipt
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TransactionReceipt {
    #[serde(rename = "blockHash")]
    pub block_hash: B256,
    #[serde(rename = "blockNumber", deserialize_with = "deserialize_u64_or_hex")]
    pub block_number: u64,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: B256,
    #[serde(
        rename = "transactionIndex",
        deserialize_with = "deserialize_u64_or_hex"
    )]
    pub transaction_index: u64,
    pub from: Address,
    pub to: Option<Address>,
    #[serde(default, deserialize_with = "deserialize_option_u64_or_hex")]
    pub status: Option<u64>,
}

/// Transaction with metadata
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TransactionWithMetadata {
    #[serde(rename = "blockHash")]
    pub block_hash: B256,
    #[serde(rename = "blockNumber", deserialize_with = "deserialize_u64_or_hex")]
    pub block_number: u64,
    #[serde(
        rename = "transactionIndex",
        deserialize_with = "deserialize_u64_or_hex"
    )]
    pub transaction_index: u64,
    pub from: Address,
    pub to: Option<Address>,
    pub hash: B256,
    #[serde(
        rename = "chainId",
        default,
        deserialize_with = "deserialize_option_u64_or_hex"
    )]
    pub chain_id: Option<u64>,
}

/// Proof response for API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofResponse {
    pub root_hash: B256,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub root_proof: Vec<u8>, // BLS aggregation proof (empty until signed)
    pub index: u32, // GeneralIndex (path encoding)
    pub leaf: B256,
    pub siblings: Vec<B256>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub original_list: Vec<B256>, // Original leaves for debugging
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_event_deserialize() {
        // OZ Monitor v1.2.0+ with payload_mode: "raw" sends MonitorMatch directly
        let json = r#"{
            "EVM": {
                "logs": [
                    {
                        "address": "0x1234567890123456789012345678901234567890",
                        "topics": ["0x0000000000000000000000000000000000000000000000000000000000000001"],
                        "data": "0x",
                        "blockNumber": 12345678,
                        "transactionHash": "0x0000000000000000000000000000000000000000000000000000000000000002",
                        "logIndex": 0
                    }
                ],
                "matched_on_args": {
                    "events": []
                },
                "monitor": {
                    "name": "Test Monitor"
                },
                "network_slug": "ethereum-mainnet",
                "transaction": {
                    "blockHash": "0x0000000000000000000000000000000000000000000000000000000000000003",
                    "blockNumber": 12345678,
                    "transactionIndex": 0,
                    "from": "0x1234567890123456789012345678901234567890",
                    "hash": "0x0000000000000000000000000000000000000000000000000000000000000002"
                }
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.evm.logs.len(), 1);
        assert_eq!(event.evm.logs[0].block_number, 12345678);
        assert_eq!(event.evm.monitor.name, "Test Monitor");
    }

    #[test]
    fn test_webhook_event_deserialize_hex_values() {
        // OZ Monitor sends blockNumber and logIndex as hex strings
        let json = r#"{
            "EVM": {
                "logs": [
                    {
                        "address": "0x1234567890123456789012345678901234567890",
                        "topics": ["0x0000000000000000000000000000000000000000000000000000000000000001"],
                        "data": "0x",
                        "blockNumber": "0x6a2",
                        "transactionHash": "0x0000000000000000000000000000000000000000000000000000000000000002",
                        "logIndex": "0x0"
                    }
                ],
                "matched_on_args": {
                    "events": []
                },
                "monitor": {
                    "name": "Test Monitor"
                },
                "network_slug": "local_anvil",
                "transaction": {
                    "blockHash": "0x0000000000000000000000000000000000000000000000000000000000000003",
                    "blockNumber": "0x6a2",
                    "transactionIndex": "0x0",
                    "from": "0x1234567890123456789012345678901234567890",
                    "hash": "0x0000000000000000000000000000000000000000000000000000000000000002",
                    "chainId": "0x7a69"
                }
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.evm.logs.len(), 1);
        assert_eq!(event.evm.logs[0].block_number, 1698); // 0x6a2
        assert_eq!(event.evm.logs[0].log_index, 0);
        assert_eq!(event.evm.transaction.as_ref().unwrap().chain_id, Some(31337)); // 0x7a69
    }

    // ============ Additional Webhook Tests ============

    #[test]
    fn test_proof_response_serialization() {
        let response = ProofResponse {
            root_hash: B256::from_slice(&[0xAAu8; 32]),
            root_proof: vec![0x01, 0x02, 0x03],
            index: 5,
            leaf: B256::from_slice(&[0xBBu8; 32]),
            siblings: vec![
                B256::from_slice(&[0x11u8; 32]),
                B256::from_slice(&[0x22u8; 32]),
            ],
            original_list: vec![
                B256::from_slice(&[0xCCu8; 32]),
            ],
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ProofResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.root_hash, response.root_hash);
        assert_eq!(deserialized.root_proof, response.root_proof);
        assert_eq!(deserialized.index, response.index);
        assert_eq!(deserialized.leaf, response.leaf);
        assert_eq!(deserialized.siblings.len(), 2);
    }

    #[test]
    fn test_proof_response_empty_fields() {
        let response = ProofResponse {
            root_hash: B256::ZERO,
            root_proof: vec![],
            index: 0,
            leaf: B256::ZERO,
            siblings: vec![],
            original_list: vec![],
        };

        let json = serde_json::to_string(&response).unwrap();

        // Empty vectors should be skipped due to skip_serializing_if
        assert!(!json.contains("root_proof"));
        assert!(!json.contains("original_list"));
    }

    #[test]
    fn test_webhook_event_empty_logs() {
        let json = r#"{
            "EVM": {
                "logs": [],
                "matched_on_args": {
                    "events": []
                },
                "monitor": {
                    "name": "Empty Test"
                },
                "network_slug": "test"
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert!(event.evm.logs.is_empty());
    }

    #[test]
    fn test_webhook_event_multiple_logs() {
        let json = r#"{
            "EVM": {
                "logs": [
                    {
                        "address": "0x1234567890123456789012345678901234567890",
                        "topics": ["0x0000000000000000000000000000000000000000000000000000000000000001"],
                        "data": "0x",
                        "blockNumber": 100,
                        "transactionHash": "0x0000000000000000000000000000000000000000000000000000000000000002",
                        "logIndex": 0
                    },
                    {
                        "address": "0xabcdef0123456789abcdef0123456789abcdef01",
                        "topics": ["0x0000000000000000000000000000000000000000000000000000000000000003"],
                        "data": "0xabcd",
                        "blockNumber": 100,
                        "transactionHash": "0x0000000000000000000000000000000000000000000000000000000000000002",
                        "logIndex": 1
                    }
                ],
                "matched_on_args": {
                    "events": []
                },
                "monitor": {
                    "name": "Multi Log Test"
                },
                "network_slug": "test"
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.evm.logs.len(), 2);
        assert_eq!(event.evm.logs[0].log_index, 0);
        assert_eq!(event.evm.logs[1].log_index, 1);
    }

    #[test]
    fn test_webhook_log_to_alloy_log() {
        let log = WebhookLog {
            address: Address::from_slice(&[0x12u8; 20]),
            topics: vec![B256::from_slice(&[0xAAu8; 32])],
            data: Bytes::from_static(&[0x01, 0x02, 0x03]),
            block_number: 12345,
            transaction_hash: B256::from_slice(&[0xBBu8; 32]),
            log_index: 7,
        };

        let alloy_log = log.to_alloy_log();

        assert_eq!(alloy_log.inner.address, log.address);
        assert_eq!(alloy_log.block_number, Some(12345));
        assert_eq!(alloy_log.transaction_hash, Some(log.transaction_hash));
        assert_eq!(alloy_log.log_index, Some(7));
        assert!(!alloy_log.removed);
    }

    #[test]
    fn test_matched_on_args_events() {
        let json = r#"{
            "EVM": {
                "logs": [],
                "matched_on_args": {
                    "events": [
                        {
                            "args": [
                                {
                                    "indexed": true,
                                    "kind": "bytes32",
                                    "name": "guid",
                                    "value": "0x1234"
                                }
                            ],
                            "hex_signature": "0xabcd",
                            "signature": "JobAssigned(bytes32)"
                        }
                    ]
                },
                "monitor": {
                    "name": "Test"
                },
                "network_slug": "test"
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.evm.matched_on_args.events.len(), 1);
        assert_eq!(event.evm.matched_on_args.events[0].args.len(), 1);
        assert!(event.evm.matched_on_args.events[0].args[0].indexed);
    }

    #[test]
    fn test_transaction_with_metadata_optional_fields() {
        let json = r#"{
            "EVM": {
                "logs": [],
                "matched_on_args": {
                    "events": []
                },
                "monitor": {
                    "name": "Test"
                },
                "network_slug": "test",
                "transaction": {
                    "blockHash": "0x0000000000000000000000000000000000000000000000000000000000000001",
                    "blockNumber": 100,
                    "transactionIndex": 0,
                    "from": "0x1234567890123456789012345678901234567890",
                    "to": null,
                    "hash": "0x0000000000000000000000000000000000000000000000000000000000000002"
                }
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        let tx = event.evm.transaction.unwrap();
        assert!(tx.to.is_none());
        assert!(tx.chain_id.is_none());
    }
}
