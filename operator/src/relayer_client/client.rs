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

        let chain_map: HashMap<u64, ChainRelayerConfig> =
            chain_configs.into_iter().map(|c| (c.chain_id, c)).collect();

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
                        let jitter_ms =
                            rand::thread_rng().gen_range(0..=base_backoff.as_millis() as u64 / 4);
                        let backoff =
                            (base_backoff + Duration::from_millis(jitter_ms)).min(MAX_BACKOFF);

                        // SAFETY: last_error is always Some when we reach this point
                        // because we only get here after setting last_error in the Err branch above
                        let err_ref = last_error.as_ref().expect("set in Err branch above");
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = self.max_retries,
                            backoff_ms = backoff.as_millis(),
                            error = %err_ref,
                            "OZ Relayer request failed, retrying"
                        );
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        // SAFETY: last_error is always Some after the retry loop completes without returning Ok
        // because we only exit the loop after setting last_error in the Err branch
        let final_error = last_error.expect("retry loop executed at least once");
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

        assert!(RelayerClient::is_retryable(&RelayerError::HttpRequest(
            "timeout".to_string()
        )));
        assert!(!RelayerClient::is_retryable(
            &RelayerError::TransactionNotFound("tx".to_string())
        ));
        assert!(!RelayerClient::is_retryable(
            &RelayerError::ChainNotConfigured(1)
        ));
        assert!(!RelayerClient::is_retryable(
            &RelayerError::MessageNotFound(B256::ZERO)
        ));
    }

    // ============ Additional Relayer Client Tests ============

    #[test]
    fn test_relayer_client_base_url_trailing_slash() {
        let configs = vec![ChainRelayerConfig::new(
            1,
            "relayer-1".to_string(),
            "0x1234".to_string(),
        )];

        let client = RelayerClient::new(
            "http://localhost:8080/".to_string(), // Trailing slash
            "test-api-key".to_string(),
            configs,
            Duration::from_secs(30),
            3,
            Duration::from_secs(1),
        )
        .unwrap();

        // Base URL should have trailing slash removed
        assert!(!client.base_url.ends_with('/'));
    }

    #[test]
    fn test_relayer_client_multiple_chains() {
        let configs = vec![
            ChainRelayerConfig::new(1, "relayer-1".to_string(), "0x1111".to_string()),
            ChainRelayerConfig::new(137, "relayer-137".to_string(), "0x2222".to_string()),
            ChainRelayerConfig::new(42161, "relayer-42161".to_string(), "0x3333".to_string()),
        ];

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
        assert!(client.get_chain_config(137).is_some());
        assert!(client.get_chain_config(42161).is_some());
        assert!(client.get_chain_config(999).is_none());
    }

    #[test]
    fn test_chain_relayer_config_new() {
        let config =
            ChainRelayerConfig::new(42161, "arb-relayer".to_string(), "0xdeadbeef".to_string());

        assert_eq!(config.chain_id, 42161);
        assert_eq!(config.relayer_id, "arb-relayer");
        assert_eq!(config.target_address, "0xdeadbeef");
    }

    #[test]
    fn test_is_retryable_501() {
        assert!(RelayerClient::is_retryable(&RelayerError::ApiError {
            status: 501,
            message: "not implemented".to_string(),
        }));
    }

    #[test]
    fn test_is_retryable_503() {
        assert!(RelayerClient::is_retryable(&RelayerError::ApiError {
            status: 503,
            message: "service unavailable".to_string(),
        }));
    }

    #[test]
    fn test_is_retryable_network_error() {
        assert!(RelayerClient::is_retryable(&RelayerError::HttpRequest(
            "connection refused".to_string()
        )));
    }

    #[test]
    fn test_is_retryable_epoch_missing() {
        assert!(!RelayerClient::is_retryable(&RelayerError::EpochMissing));
    }

    #[test]
    fn test_is_retryable_proof_generation() {
        assert!(!RelayerClient::is_retryable(
            &RelayerError::ProofGeneration("failed".to_string())
        ));
    }

    #[test]
    fn test_get_chain_config_returns_reference() {
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

        let config = client.get_chain_config(1).unwrap();
        assert_eq!(config.relayer_id, "relayer-1");
        assert_eq!(config.target_address, "0x1234");
    }

    #[test]
    fn test_relayer_error_display() {
        let err = RelayerError::ApiError {
            status: 429,
            message: "rate limited".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("429"));
        assert!(display.contains("rate limited"));
    }

    #[test]
    fn test_relayer_error_chain_not_configured() {
        let err = RelayerError::ChainNotConfigured(42161);
        assert!(err.to_string().contains("42161"));
    }

    #[test]
    fn test_relayer_error_transaction_not_found() {
        let err = RelayerError::TransactionNotFound("tx-123".to_string());
        assert!(err.to_string().contains("tx-123"));
    }

    #[tokio::test]
    async fn test_retry_with_backoff_retries_then_ok() {
        let configs = vec![ChainRelayerConfig::new(
            1,
            "relayer-1".to_string(),
            "0x1234".to_string(),
        )];

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            configs,
            Duration::from_secs(1),
            1,
            Duration::from_millis(0),
        )
        .unwrap();

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<u32, RelayerError> = client
            .retry_with_backoff(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count == 0 {
                        Err(RelayerError::ApiError {
                            status: 500,
                            message: "server error".to_string(),
                        })
                    } else {
                        Ok(42)
                    }
                }
            })
            .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_max_retries_exhausted() {
        let configs = vec![ChainRelayerConfig::new(
            1,
            "relayer-1".to_string(),
            "0x1234".to_string(),
        )];

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            configs,
            Duration::from_secs(1),
            2, // max 2 retries = 3 total attempts
            Duration::from_millis(0),
        )
        .unwrap();

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<u32, RelayerError> = client
            .retry_with_backoff(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(RelayerError::ApiError {
                        status: 500,
                        message: "server error".to_string(),
                    })
                }
            })
            .await;

        // Should have attempted 3 times (initial + 2 retries)
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RelayerError::ApiError { status: 500, .. }
        ));
    }

    #[tokio::test]
    async fn test_retry_with_backoff_non_retryable() {
        let configs = vec![ChainRelayerConfig::new(
            1,
            "relayer-1".to_string(),
            "0x1234".to_string(),
        )];

        let client = RelayerClient::new(
            "http://localhost:8080".to_string(),
            "test-api-key".to_string(),
            configs,
            Duration::from_secs(1),
            2,
            Duration::from_millis(0),
        )
        .unwrap();

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<u32, RelayerError> = client
            .retry_with_backoff(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(RelayerError::ApiError {
                        status: 400,
                        message: "bad request".to_string(),
                    })
                }
            })
            .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(result.is_err());
    }
}
