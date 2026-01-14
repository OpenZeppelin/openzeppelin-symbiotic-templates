//! HTTP client for OpenZeppelin Relayer API

use std::collections::HashMap;
use std::time::Duration;

use rand::Rng;
use reqwest::{Client, StatusCode};

use crate::error::RelayerError;

/// Maximum backoff duration to prevent excessive waits (60 seconds)
const MAX_BACKOFF: Duration = Duration::from_secs(60);

use super::types::{
    ChainRelayerConfig, CreateTransactionResponse, EvmTransactionRequest, TransactionResponse,
};

/// OpenZeppelin Relayer HTTP client
#[derive(Clone)]
pub struct RelayerClient {
    http_client: Client,
    base_url: String,
    api_key: String,
    /// Map from chain_id to relayer config
    chain_configs: HashMap<u64, ChainRelayerConfig>,
    /// Maximum number of retry attempts for transient errors
    max_retries: u32,
    /// Base backoff duration for retries (exponential: backoff * 2^attempt)
    retry_backoff: Duration,
}

impl RelayerClient {
    /// Create a new RelayerClient
    pub fn new(
        base_url: String,
        api_key: String,
        chain_configs: Vec<ChainRelayerConfig>,
        timeout: Duration,
        max_retries: u32,
        retry_backoff: Duration,
    ) -> Result<Self, RelayerError> {
        let http_client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| RelayerError::HttpClient(e.to_string()))?;

        let chain_map: HashMap<u64, ChainRelayerConfig> = chain_configs
            .into_iter()
            .map(|c| (c.chain_id, c))
            .collect();

        Ok(Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            chain_configs: chain_map,
            max_retries,
            retry_backoff,
        })
    }

    /// Get config for a chain
    pub fn get_chain_config(&self, chain_id: u64) -> Option<&ChainRelayerConfig> {
        self.chain_configs.get(&chain_id)
    }

    /// Send a transaction via OZ Relayer with retry logic
    ///
    /// POST /api/v1/relayers/{relayer_id}/transactions
    pub async fn send_transaction(
        &self,
        chain_id: u64,
        request: EvmTransactionRequest,
    ) -> Result<CreateTransactionResponse, RelayerError> {
        let config = self
            .chain_configs
            .get(&chain_id)
            .ok_or(RelayerError::ChainNotConfigured(chain_id))?;

        let url = format!(
            "{}/api/v1/relayers/{}/transactions",
            self.base_url, config.relayer_id
        );

        tracing::debug!(
            chain_id,
            relayer_id = %config.relayer_id,
            to = %request.to,
            "sending transaction to OZ Relayer"
        );

        self.retry_with_backoff(|| async {
            let response = self
                .http_client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send()
                .await
                .map_err(|e| RelayerError::HttpRequest(e.to_string()))?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(RelayerError::ApiError {
                    status: status.as_u16(),
                    message: body,
                });
            }

            response
                .json()
                .await
                .map_err(|e| RelayerError::HttpRequest(format!("failed to parse response: {}", e)))
        })
        .await
    }

    /// Get transaction status by ID with retry logic
    ///
    /// GET /api/v1/relayers/{relayer_id}/transactions/{tx_id}
    pub async fn get_transaction(
        &self,
        chain_id: u64,
        tx_id: &str,
    ) -> Result<TransactionResponse, RelayerError> {
        let config = self
            .chain_configs
            .get(&chain_id)
            .ok_or(RelayerError::ChainNotConfigured(chain_id))?;

        let url = format!(
            "{}/api/v1/relayers/{}/transactions/{}",
            self.base_url, config.relayer_id, tx_id
        );

        self.retry_with_backoff(|| async {
            let response = self
                .http_client
                .get(&url)
                .bearer_auth(&self.api_key)
                .send()
                .await
                .map_err(|e| RelayerError::HttpRequest(e.to_string()))?;

            let status = response.status();
            if status == StatusCode::NOT_FOUND {
                return Err(RelayerError::TransactionNotFound(tx_id.to_string()));
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(RelayerError::ApiError {
                    status: status.as_u16(),
                    message: body,
                });
            }

            response
                .json()
                .await
                .map_err(|e| RelayerError::HttpRequest(format!("failed to parse response: {}", e)))
        })
        .await
    }

    /// Retry with exponential backoff and jitter
    async fn retry_with_backoff<F, Fut, T>(&self, f: F) -> Result<T, RelayerError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, RelayerError>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match f().await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if !Self::is_retryable(&err) {
                        return Err(err);
                    }
                    last_error = Some(err);

                    if attempt < self.max_retries {
                        let multiplier = 2u32.saturating_pow(attempt);
                        let base_backoff = self.retry_backoff.saturating_mul(multiplier);
                        let jitter_ms = rand::thread_rng().gen_range(0..=base_backoff.as_millis() as u64 / 4);
                        let backoff = (base_backoff + Duration::from_millis(jitter_ms)).min(MAX_BACKOFF);

                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = self.max_retries,
                            backoff_ms = backoff.as_millis(),
                            error = %last_error.as_ref().unwrap(),
                            "OZ Relayer request failed, retrying"
                        );
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        let final_error = last_error.unwrap();
        tracing::error!(
            max_retries = self.max_retries,
            error = %final_error,
            "OZ Relayer request failed after all retries exhausted"
        );
        Err(final_error)
    }

    /// Retryable: 429, 500-504, network errors. Non-retryable: other 4xx, domain errors.
    fn is_retryable(error: &RelayerError) -> bool {
        match error {
            RelayerError::HttpRequest(_) => true,
            RelayerError::ApiError { status, .. } => *status == 429 || (500..=504).contains(status),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;

    #[test]
    fn test_relayer_client_creation() {
        let configs = vec![ChainRelayerConfig::new(
            1,
            "relayer-1".to_string(),
            "0x1234".to_string(),
        )];

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            configs,
            Duration::from_secs(30),
            3,
            Duration::from_secs(1),
        )
        .unwrap();

        assert!(client.get_chain_config(1).is_some());
        assert!(client.get_chain_config(999).is_none());
    }

    #[test]
    fn test_is_retryable_errors() {
        for status in [429, 500, 502, 503, 504] {
            assert!(
                RelayerClient::is_retryable(&RelayerError::ApiError {
                    status,
                    message: "test".to_string(),
                }),
                "status {status} should be retryable"
            );
        }

        for status in [400, 401, 403, 404, 505] {
            assert!(
                !RelayerClient::is_retryable(&RelayerError::ApiError {
                    status,
                    message: "test".to_string(),
                }),
                "status {status} should not be retryable"
            );
        }

        assert!(RelayerClient::is_retryable(&RelayerError::HttpRequest("timeout".to_string())));
        assert!(!RelayerClient::is_retryable(&RelayerError::TransactionNotFound("tx".to_string())));
        assert!(!RelayerClient::is_retryable(&RelayerError::ChainNotConfigured(1)));
        assert!(!RelayerClient::is_retryable(&RelayerError::MessageNotFound(B256::ZERO)));
    }
}
