use tonic::transport::Channel;

use crate::config::SymbioticRelayConfig;
use crate::error::SymbioticRelayError;

pub mod symbiotic_relay_proto {
    tonic::include_proto!("api.proto.v1");
}

pub use symbiotic_relay_proto::symbiotic_api_service_client::SymbioticApiServiceClient;
pub use symbiotic_relay_proto::{
    AggregationProof, GetAggregationProofRequest, GetAggregationProofResponse,
    GetLastAllCommittedRequest, SignMessageRequest, SignMessageResponse,
};

/// Client for communicating with the Symbiotic relay network for BLS signature aggregation
#[derive(Clone)]
pub struct SymbioticRelayClient {
    inner: SymbioticApiServiceClient<Channel>,
    config: SymbioticRelayConfig,
}

impl SymbioticRelayClient {
    /// Create a new Symbiotic relay client
    pub async fn new(config: SymbioticRelayConfig) -> Result<Self, SymbioticRelayError> {
        let channel = create_symbiotic_relay_channel(&config).await?;
        Ok(Self {
            inner: SymbioticApiServiceClient::new(channel),
            config,
        })
    }

    /// Get the suggested committed epoch from the relay
    /// Returns None if no epochs are committed yet
    pub async fn get_committed_epoch(&mut self) -> Result<Option<u64>, SymbioticRelayError> {
        let req = GetLastAllCommittedRequest {};

        let resp = self
            .retry_with_backoff(|| async {
                let mut client = self.inner.clone();
                client.get_last_all_committed(req.clone()).await
            })
            .await?;

        let inner = resp.into_inner();

        // Use the pre-computed suggested epoch (minimum across all chains)
        let epoch = inner
            .suggested_epoch_info
            .map(|info| info.last_committed_epoch);

        if let Some(e) = epoch {
            tracing::debug!(epoch = e, "using suggested committed epoch for signing");
        } else {
            tracing::warn!("no committed epochs found, signing without required_epoch");
        }

        Ok(epoch)
    }

    /// Sign a message (merkle root hash) using the latest committed epoch
    pub async fn sign_message(
        &mut self,
        message: &[u8],
        key_tag: u32,
    ) -> Result<SignMessageResponse, SymbioticRelayError> {
        // First, get the committed epoch to ensure on-chain verifiability
        let required_epoch = self.get_committed_epoch().await?;

        let req = SignMessageRequest {
            key_tag,
            message: message.to_vec(),
            required_epoch,
        };

        let resp = self
            .retry_with_backoff(|| async {
                let mut client = self.inner.clone();
                client.sign_message(req.clone()).await
            })
            .await?;

        Ok(resp.into_inner())
    }

    /// Get aggregation proof for a signing request
    pub async fn get_aggregation_proof(
        &mut self,
        request_id: &str,
    ) -> Result<GetAggregationProofResponse, SymbioticRelayError> {
        let req = GetAggregationProofRequest {
            request_id: request_id.to_string(),
        };

        let result = self
            .retry_with_backoff(|| async {
                let mut client = self.inner.clone();
                client.get_aggregation_proof(req.clone()).await
            })
            .await;

        match result {
            Ok(resp) => Ok(resp.into_inner()),
            Err(SymbioticRelayError::Rpc(status)) if status.code() == tonic::Code::NotFound => {
                Err(SymbioticRelayError::NotReady)
            }
            Err(e) => Err(e),
        }
    }

    /// Retry with linear backoff
    async fn retry_with_backoff<F, Fut, T>(&self, f: F) -> Result<T, SymbioticRelayError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, tonic::Status>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            match f().await {
                Ok(result) => return Ok(result),
                Err(status) => {
                    last_error = Some(status.clone());

                    // Don't retry on non-retryable errors
                    if !Self::is_retryable(&status) {
                        return Err(SymbioticRelayError::Rpc(status));
                    }

                    if attempt < self.config.max_retries {
                        let backoff = self.config.retry_backoff * (attempt + 1);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = self.config.max_retries,
                            backoff_ms = backoff.as_millis(),
                            error = %status,
                            "relay request failed, retrying"
                        );
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(SymbioticRelayError::Rpc(
            last_error.expect("retry loop executed at least once"),
        ))
    }

    /// Check if an error is retryable
    fn is_retryable(status: &tonic::Status) -> bool {
        matches!(
            status.code(),
            tonic::Code::Unavailable
                | tonic::Code::DeadlineExceeded
                | tonic::Code::ResourceExhausted
                | tonic::Code::Aborted
                | tonic::Code::Internal
        )
    }
}

/// Create a gRPC channel with appropriate configuration
async fn create_symbiotic_relay_channel(
    config: &SymbioticRelayConfig,
) -> Result<Channel, SymbioticRelayError> {
    let endpoint = tonic::transport::Endpoint::from_shared(config.address.clone())
        .map_err(|_| SymbioticRelayError::InvalidAddress(config.address.clone()))?
        .timeout(config.timeout)
        // Large message sizes for aggregation proofs
        .initial_stream_window_size(100 * 1024 * 1024) // 100MB
        .initial_connection_window_size(100 * 1024 * 1024);

    let channel = endpoint.connect().await?;
    Ok(channel)
}

/// Mock relay client for testing
#[derive(Clone)]
pub struct MockSymbioticRelayClient {
    request_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl MockSymbioticRelayClient {
    pub fn new() -> Self {
        Self {
            request_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub async fn sign_message(
        &mut self,
        message: &[u8],
        _key_tag: u32,
    ) -> Result<SignMessageResponse, SymbioticRelayError> {
        let count = self
            .request_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(
            message_hex = hex::encode(message),
            request_id = count,
            "mock: sign_message called"
        );

        Ok(SignMessageResponse {
            request_id: format!("mock-request-{}", count),
            epoch: 1,
        })
    }

    pub async fn get_aggregation_proof(
        &mut self,
        request_id: &str,
    ) -> Result<GetAggregationProofResponse, SymbioticRelayError> {
        tracing::info!(request_id, "mock: get_aggregation_proof called");

        // Return a mock proof
        Ok(GetAggregationProofResponse {
            aggregation_proof: Some(AggregationProof {
                message_hash: vec![0u8; 32],
                proof: vec![0u8; 96], // Mock BLS signature
                request_id: request_id.to_string(),
            }),
        })
    }
}

impl Default for MockSymbioticRelayClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Relay client enum for runtime dispatch
#[derive(Clone)]
pub enum SymbioticRelayClientEnum {
    Real(SymbioticRelayClient),
    Mock(MockSymbioticRelayClient),
}

impl SymbioticRelayClientEnum {
    pub async fn sign_message(
        &mut self,
        message: &[u8],
        key_tag: u32,
    ) -> Result<SignMessageResponse, SymbioticRelayError> {
        match self {
            SymbioticRelayClientEnum::Real(client) => client.sign_message(message, key_tag).await,
            SymbioticRelayClientEnum::Mock(client) => client.sign_message(message, key_tag).await,
        }
    }

    pub async fn get_aggregation_proof(
        &mut self,
        request_id: &str,
    ) -> Result<GetAggregationProofResponse, SymbioticRelayError> {
        match self {
            SymbioticRelayClientEnum::Real(client) => {
                client.get_aggregation_proof(request_id).await
            }
            SymbioticRelayClientEnum::Mock(client) => {
                client.get_aggregation_proof(request_id).await
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ============ Mock Client Tests ============

    #[test]
    fn test_mock_client_new() {
        let client = MockSymbioticRelayClient::new();
        // Counter should start at 0
        assert_eq!(
            client
                .request_counter
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn test_mock_client_default() {
        let client = MockSymbioticRelayClient::default();
        assert_eq!(
            client
                .request_counter
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn test_mock_client_sign_message() {
        let mut client = MockSymbioticRelayClient::new();
        let message = b"test message";
        let key_tag = 15;

        let response = client.sign_message(message, key_tag).await.unwrap();

        // Should return a valid response with request_id
        assert!(response.request_id.starts_with("mock-request-"));
        assert_eq!(response.epoch, 1);

        // Counter should increment
        assert_eq!(
            client
                .request_counter
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn test_mock_client_sign_message_increments_counter() {
        let mut client = MockSymbioticRelayClient::new();

        let r1 = client.sign_message(b"msg1", 15).await.unwrap();
        let r2 = client.sign_message(b"msg2", 15).await.unwrap();
        let r3 = client.sign_message(b"msg3", 15).await.unwrap();

        // Request IDs should be unique and incrementing
        assert_eq!(r1.request_id, "mock-request-0");
        assert_eq!(r2.request_id, "mock-request-1");
        assert_eq!(r3.request_id, "mock-request-2");
    }

    #[tokio::test]
    async fn test_mock_client_get_aggregation_proof() {
        let mut client = MockSymbioticRelayClient::new();
        let request_id = "test-request-123";

        let response = client.get_aggregation_proof(request_id).await.unwrap();

        // Should return a proof with the same request_id
        assert!(response.aggregation_proof.is_some());
        let proof = response.aggregation_proof.unwrap();
        assert_eq!(proof.request_id, request_id);
        assert_eq!(proof.proof.len(), 96); // Mock BLS signature size
        assert_eq!(proof.message_hash.len(), 32);
    }

    // ============ Enum Dispatch Tests ============

    #[tokio::test]
    async fn test_client_enum_dispatch_sign_message() {
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        let response = client.sign_message(b"test", 15).await.unwrap();
        assert!(response.request_id.starts_with("mock-request-"));
    }

    #[tokio::test]
    async fn test_client_enum_dispatch_get_proof() {
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        let response = client.get_aggregation_proof("test-id").await.unwrap();
        assert!(response.aggregation_proof.is_some());
    }

    #[test]
    fn test_client_enum_clone() {
        let client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());
        let cloned = client.clone();

        // Both should work (cloning Mock is fine)
        match cloned {
            SymbioticRelayClientEnum::Mock(_) => {}
            _ => panic!("expected Mock variant"),
        }
    }

    // ============ Retry Logic Tests ============

    #[test]
    fn test_is_retryable_unavailable() {
        let status = tonic::Status::unavailable("service unavailable");
        assert!(SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_deadline_exceeded() {
        let status = tonic::Status::deadline_exceeded("timeout");
        assert!(SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_resource_exhausted() {
        let status = tonic::Status::resource_exhausted("rate limited");
        assert!(SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_aborted() {
        let status = tonic::Status::aborted("aborted");
        assert!(SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_internal() {
        let status = tonic::Status::internal("internal error");
        assert!(SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_not_found() {
        let status = tonic::Status::not_found("not found");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_invalid_argument() {
        let status = tonic::Status::invalid_argument("bad request");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_permission_denied() {
        let status = tonic::Status::permission_denied("forbidden");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_unauthenticated() {
        let status = tonic::Status::unauthenticated("no auth");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_ok() {
        let status = tonic::Status::ok("ok");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_cancelled() {
        let status = tonic::Status::cancelled("cancelled");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_unknown() {
        let status = tonic::Status::unknown("unknown");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_already_exists() {
        let status = tonic::Status::already_exists("already exists");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_failed_precondition() {
        let status = tonic::Status::failed_precondition("precondition");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_out_of_range() {
        let status = tonic::Status::out_of_range("range");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_unimplemented() {
        let status = tonic::Status::unimplemented("unimplemented");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    #[test]
    fn test_is_retryable_data_loss() {
        let status = tonic::Status::data_loss("data loss");
        assert!(!SymbioticRelayClient::is_retryable(&status));
    }

    // ============ Error Type Tests ============

    #[test]
    fn test_symbiotic_relay_error_not_ready() {
        let err = crate::error::SymbioticRelayError::NotReady;
        assert_eq!(err.to_string(), "proof not ready");
    }

    #[test]
    fn test_symbiotic_relay_error_invalid_address() {
        let err = crate::error::SymbioticRelayError::InvalidAddress("bad-address".to_string());
        assert!(err.to_string().contains("bad-address"));
    }

    #[tokio::test]
    async fn test_symbiotic_relay_client_new_invalid_address() {
        let config = SymbioticRelayConfig {
            address: "not-a-url".to_string(),
            key_tag: 15,
            use_mock: false,
            max_retries: 1,
            timeout: std::time::Duration::from_secs(1),
            retry_backoff: std::time::Duration::from_millis(1),
        };

        let result = SymbioticRelayClient::new(config).await;
        assert!(result.is_err());
    }

    // ============ Proto Type Tests ============

    #[test]
    fn test_sign_message_response_fields() {
        let response = SignMessageResponse {
            request_id: "req-123".to_string(),
            epoch: 42,
        };
        assert_eq!(response.request_id, "req-123");
        assert_eq!(response.epoch, 42);
    }

    #[test]
    fn test_aggregation_proof_fields() {
        let proof = AggregationProof {
            message_hash: vec![0u8; 32],
            proof: vec![0u8; 96],
            request_id: "req-456".to_string(),
        };
        assert_eq!(proof.message_hash.len(), 32);
        assert_eq!(proof.proof.len(), 96);
        assert_eq!(proof.request_id, "req-456");
    }

    #[test]
    fn test_get_aggregation_proof_response_some() {
        let response = GetAggregationProofResponse {
            aggregation_proof: Some(AggregationProof {
                message_hash: vec![],
                proof: vec![],
                request_id: "test".to_string(),
            }),
        };
        assert!(response.aggregation_proof.is_some());
    }

    #[tokio::test]
    async fn test_create_channel_invalid_address_empty() {
        let config = SymbioticRelayConfig {
            address: "".to_string(),
            key_tag: 15,
            use_mock: false,
            max_retries: 1,
            timeout: std::time::Duration::from_secs(1),
            retry_backoff: std::time::Duration::from_millis(1),
        };

        let result = SymbioticRelayClient::new(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_client_enum_sign_multiple_messages() {
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        let r1 = client.sign_message(b"msg1", 15).await.unwrap();
        let r2 = client.sign_message(b"msg2", 20).await.unwrap();

        assert_ne!(r1.request_id, r2.request_id);
        assert_eq!(r1.epoch, 1);
        assert_eq!(r2.epoch, 1);
    }

    #[tokio::test]
    async fn test_mock_client_enum_get_aggregation_proof_returns_proof() {
        let mut client = SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new());

        let response = client.get_aggregation_proof("req-abc").await.unwrap();
        let proof = response.aggregation_proof.unwrap();
        assert_eq!(proof.request_id, "req-abc");
        assert_eq!(proof.proof.len(), 96);
        assert_eq!(proof.message_hash.len(), 32);
    }

    #[test]
    fn test_symbiotic_relay_error_connection() {
        let err = crate::error::SymbioticRelayError::InvalidAddress("http://bad".to_string());
        assert!(err.to_string().contains("http://bad"));
    }

    #[test]
    fn test_sign_message_request_fields() {
        let req = SignMessageRequest {
            key_tag: 42,
            message: vec![1, 2, 3],
            required_epoch: Some(10),
        };
        assert_eq!(req.key_tag, 42);
        assert_eq!(req.message, vec![1, 2, 3]);
        assert_eq!(req.required_epoch, Some(10));
    }

    #[test]
    fn test_sign_message_request_no_epoch() {
        let req = SignMessageRequest {
            key_tag: 15,
            message: vec![],
            required_epoch: None,
        };
        assert!(req.required_epoch.is_none());
    }

    #[test]
    fn test_get_last_all_committed_request() {
        let req = GetLastAllCommittedRequest {};
        // Just verify it can be constructed (empty message)
        drop(req);
    }

    #[test]
    fn test_get_aggregation_proof_request_fields() {
        let req = GetAggregationProofRequest {
            request_id: "test-123".to_string(),
        };
        assert_eq!(req.request_id, "test-123");
    }

    #[test]
    fn test_mock_client_clone_shares_counter() {
        let client1 = MockSymbioticRelayClient::new();
        let client2 = client1.clone();

        // Increment on one clone
        client1
            .request_counter
            .fetch_add(5, std::sync::atomic::Ordering::SeqCst);

        // Should be visible on the other
        assert_eq!(
            client2
                .request_counter
                .load(std::sync::atomic::Ordering::SeqCst),
            5
        );
    }

    #[test]
    fn test_get_aggregation_proof_response_none() {
        let response = GetAggregationProofResponse {
            aggregation_proof: None,
        };
        assert!(response.aggregation_proof.is_none());
    }
}
