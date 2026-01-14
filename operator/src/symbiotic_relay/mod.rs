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
async fn create_symbiotic_relay_channel(config: &SymbioticRelayConfig) -> Result<Channel, SymbioticRelayError> {
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
            SymbioticRelayClientEnum::Real(client) => client.get_aggregation_proof(request_id).await,
            SymbioticRelayClientEnum::Mock(client) => client.get_aggregation_proof(request_id).await,
        }
    }
}
