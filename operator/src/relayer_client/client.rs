//! HTTP client for OpenZeppelin Relayer API

use std::collections::HashMap;
use std::time::Duration;

use reqwest::{Client, StatusCode};

use crate::error::RelayerError;

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
}

impl RelayerClient {
    /// Create a new RelayerClient
    pub fn new(
        base_url: String,
        api_key: String,
        chain_configs: Vec<ChainRelayerConfig>,
        timeout: Duration,
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
        })
    }

    /// Get config for a chain
    pub fn get_chain_config(&self, chain_id: u64) -> Option<&ChainRelayerConfig> {
        self.chain_configs.get(&chain_id)
    }

    /// Send a transaction via OZ Relayer
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
    }

    /// Get transaction status by ID
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
    }

}

#[cfg(test)]
mod tests {
    use super::*;

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
        )
        .unwrap();

        assert!(client.get_chain_config(1).is_some());
        assert!(client.get_chain_config(999).is_none());
    }
}
