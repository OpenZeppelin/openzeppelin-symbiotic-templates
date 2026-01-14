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

/// Top-level webhook event from OZ Monitor
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct WebhookEvent {
    pub args: serde_json::Value,
    pub monitor_match: MonitorMatch,
}

/// Monitor match data
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorMatch {
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
        let json = r#"{
            "args": {},
            "monitor_match": {
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
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.monitor_match.evm.logs.len(), 1);
        assert_eq!(event.monitor_match.evm.logs[0].block_number, 12345678);
        assert_eq!(event.monitor_match.evm.monitor.name, "Test Monitor");
    }

    #[test]
    fn test_webhook_event_deserialize_hex_values() {
        // OZ Monitor sends blockNumber and logIndex as hex strings
        let json = r#"{
            "args": {},
            "monitor_match": {
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
            }
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.monitor_match.evm.logs.len(), 1);
        assert_eq!(event.monitor_match.evm.logs[0].block_number, 1698); // 0x6a2
        assert_eq!(event.monitor_match.evm.logs[0].log_index, 0);
        assert_eq!(
            event
                .monitor_match
                .evm
                .transaction
                .as_ref()
                .unwrap()
                .chain_id,
            Some(31337) // 0x7a69
        );
    }
}
