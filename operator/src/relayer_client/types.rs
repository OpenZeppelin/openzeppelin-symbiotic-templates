//! Types for OpenZeppelin Relayer HTTP API

use serde::{Deserialize, Serialize};

/// Transaction speed for gas pricing
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Speed {
    Fastest,
    #[default]
    Fast,
    Average,
    SafeLow,
}

impl std::str::FromStr for Speed {
    type Err = std::convert::Infallible;

    /// Parse speed from string (case-insensitive), defaults to Fast
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "fastest" => Speed::Fastest,
            "fast" => Speed::Fast,
            "average" => Speed::Average,
            "safelow" | "safe_low" => Speed::SafeLow,
            _ => Speed::Fast,
        })
    }
}

/// Request to send an EVM transaction via OZ Relayer
#[derive(Debug, Clone, Serialize)]
pub struct EvmTransactionRequest {
    /// Target contract address (hex with 0x prefix)
    pub to: String,
    /// Calldata (hex with 0x prefix)
    pub data: String,
    /// Value to send (as string, "0" for contract calls)
    pub value: String,
    /// Gas speed tier
    pub speed: Speed,
    /// Optional gas limit override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit: Option<u64>,
    /// Idempotency key for deduplication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl EvmTransactionRequest {
    /// Create a new transaction request
    pub fn new(to: String, data: String, speed: Speed) -> Self {
        Self {
            to,
            data,
            value: "0".to_string(),
            speed,
            gas_limit: None,
            idempotency_key: None,
        }
    }

    /// Set idempotency key
    pub fn with_idempotency_key(mut self, key: String) -> Self {
        self.idempotency_key = Some(key);
        self
    }
}

/// Transaction status from OZ Relayer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    /// Transaction is queued but not yet sent
    Pending,
    /// Transaction has been sent to the network
    Sent,
    /// Transaction has been submitted (may be pending confirmation)
    Submitted,
    /// Transaction has been mined
    Mined,
    /// Transaction has been confirmed
    Confirmed,
    /// Transaction failed
    Failed,
    /// Transaction was canceled
    Canceled,
    /// Transaction expired
    Expired,
}

impl TransactionStatus {
    /// Check if this status is terminal (no more updates expected)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TransactionStatus::Confirmed
                | TransactionStatus::Failed
                | TransactionStatus::Canceled
                | TransactionStatus::Expired
        )
    }
}

/// Transaction response from OZ Relayer
/// Note: Some fields are only used for deserialization from the API
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TransactionResponse {
    /// Internal transaction ID from OZ Relayer
    pub id: String,
    /// Transaction hash (once sent to network)
    pub hash: Option<String>,
    /// Current status
    pub status: TransactionStatus,
    /// Nonce used
    pub nonce: Option<u64>,
    /// Creation timestamp
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    /// Timestamp when sent to network
    #[serde(rename = "sentAt")]
    pub sent_at: Option<String>,
    /// Timestamp when confirmed
    #[serde(rename = "confirmedAt")]
    pub confirmed_at: Option<String>,
    /// Reason for status (especially useful for failures)
    #[serde(rename = "statusReason")]
    pub status_reason: Option<String>,
}

/// Wrapper for OZ Relayer API responses
#[derive(Debug, Clone, Deserialize)]
pub struct RelayerApiResponse<T> {
    pub success: bool,
    pub data: T,
    #[allow(dead_code)]
    pub error: Option<String>,
}

/// Transaction data returned when creating a transaction
#[derive(Debug, Clone, Deserialize)]
pub struct TransactionData {
    /// Internal transaction ID
    pub id: String,
}

/// Response when creating a transaction (wrapped in RelayerApiResponse)
pub type CreateTransactionResponse = RelayerApiResponse<TransactionData>;

/// Chain to relayer ID mapping
#[derive(Debug, Clone)]
pub struct ChainRelayerConfig {
    /// EVM chain ID
    pub chain_id: u64,
    /// OZ Relayer ID for this chain
    pub relayer_id: String,
    /// DVN contract address on this chain
    pub dvn_address: String,
}

impl ChainRelayerConfig {
    pub fn new(chain_id: u64, relayer_id: String, dvn_address: String) -> Self {
        Self {
            chain_id,
            relayer_id,
            dvn_address,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_speed_serialization() {
        let speed = Speed::Fast;
        let json = serde_json::to_string(&speed).unwrap();
        assert_eq!(json, "\"fast\"");

        let speed = Speed::SafeLow;
        let json = serde_json::to_string(&speed).unwrap();
        assert_eq!(json, "\"safeLow\"");
    }

    #[test]
    fn test_speed_from_str() {
        assert!(matches!("fast".parse::<Speed>().unwrap(), Speed::Fast));
        assert!(matches!("FAST".parse::<Speed>().unwrap(), Speed::Fast));
        assert!(matches!("safelow".parse::<Speed>().unwrap(), Speed::SafeLow));
        assert!(matches!("safe_low".parse::<Speed>().unwrap(), Speed::SafeLow));
        assert!(matches!("unknown".parse::<Speed>().unwrap(), Speed::Fast)); // default
    }

    #[test]
    fn test_transaction_status() {
        assert!(TransactionStatus::Confirmed.is_terminal());
        assert!(TransactionStatus::Failed.is_terminal());
        assert!(!TransactionStatus::Pending.is_terminal());
        assert!(!TransactionStatus::Sent.is_terminal());
    }

    #[test]
    fn test_transaction_request_builder() {
        let req = EvmTransactionRequest::new(
            "0x1234".to_string(),
            "0xabcd".to_string(),
            Speed::Fast,
        )
        .with_idempotency_key("test-key".to_string());

        assert_eq!(req.to, "0x1234");
        assert_eq!(req.data, "0xabcd");
        assert_eq!(req.idempotency_key, Some("test-key".to_string()));
    }

    #[test]
    fn test_create_transaction_response_parsing() {
        // Real response format from OZ Relayer API
        let response_json = r#"{
            "success": true,
            "data": {
                "id": "84b29a3d-106f-4d65-a842-6c83aa8af05e",
                "hash": null,
                "status": "pending",
                "status_reason": null,
                "created_at": "2026-01-26T22:38:03.380436878+00:00",
                "sent_at": null,
                "confirmed_at": null,
                "gas_price": null,
                "gas_limit": null,
                "nonce": null,
                "value": "0x0",
                "from": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
                "to": "0x5eb3Bc0a489C5A8288765d2336659EbCA68FCd00",
                "relayer_id": "dvn-relayer-1",
                "data": "0x1234",
                "speed": "fast"
            },
            "error": null
        }"#;

        let response: CreateTransactionResponse = serde_json::from_str(response_json).unwrap();

        assert!(response.success);
        assert_eq!(response.data.id, "84b29a3d-106f-4d65-a842-6c83aa8af05e");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_create_transaction_response_with_error() {
        let response_json = r#"{
            "success": false,
            "data": {
                "id": ""
            },
            "error": "insufficient funds"
        }"#;

        let response: CreateTransactionResponse = serde_json::from_str(response_json).unwrap();

        assert!(!response.success);
        assert_eq!(response.error, Some("insufficient funds".to_string()));
    }
}
