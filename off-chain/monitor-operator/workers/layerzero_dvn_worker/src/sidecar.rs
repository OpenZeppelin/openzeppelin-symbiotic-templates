use anyhow::{anyhow, Result};
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::{debug, info, warn};

// Include generated protobuf code
pub mod proto {
    tonic::include_proto!("api.proto.v1");
}

use proto::symbiotic_api_service_client::SymbioticApiServiceClient as GrpcSidecarClient;
use proto::{GetCurrentEpochRequest, SignMessageWaitRequest, SigningStatus};

/// Key tag for BLS-BN254 signatures (standard Symbiotic key tag)
pub const KEY_TAG_BLS_BN254: u8 = 15;

/// Result from signing with aggregation proof
#[derive(Debug, Clone)]
pub struct SignWithProofResult {
    pub request_id: String,
    pub epoch: u64,
    pub message_hash: Vec<u8>,
    pub proof: Vec<u8>,
}

/// Symbiotic Relay Sidecar gRPC Client
pub struct SidecarClient {
    endpoint: String,
}

impl SidecarClient {
    pub fn new(sidecar_url: &str) -> Self {
        // Convert http:// URL to proper format for tonic
        let endpoint = sidecar_url.trim_end_matches('/').to_string();
        Self { endpoint }
    }

    /// Create a gRPC channel connection
    async fn connect(&self) -> Result<GrpcSidecarClient<Channel>> {
        let channel = Channel::from_shared(self.endpoint.clone())?
            .connect()
            .await?;
        Ok(GrpcSidecarClient::new(channel))
    }

    /// Sign a message and wait for aggregation proof (streaming)
    /// This is the main method - uses SignMessageWait which streams status updates
    /// and returns the aggregation proof when complete.
    pub async fn sign_message_wait(
        &self,
        key_tag: u8,
        message: &[u8],
    ) -> Result<SignWithProofResult> {
        let mut client = self.connect().await?;

        let request = SignMessageWaitRequest {
            key_tag: key_tag as u32,
            message: message.to_vec(),
            required_epoch: None,
        };

        debug!("Sending SignMessageWait request with key_tag={}", key_tag);

        let response = client.sign_message_wait(request).await?;
        let mut stream = response.into_inner();

        let mut result: Option<SignWithProofResult> = None;

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            let status = SigningStatus::try_from(msg.status)
                .unwrap_or(SigningStatus::Unspecified);

            debug!(
                "SignMessageWait status: {:?}, request_id: {}, epoch: {}",
                status, msg.request_id, msg.epoch
            );

            match status {
                SigningStatus::Completed => {
                    if let Some(proof) = msg.aggregation_proof {
                        info!(
                            "Signing completed! request_id: {}, epoch: {}",
                            msg.request_id, msg.epoch
                        );
                        result = Some(SignWithProofResult {
                            request_id: msg.request_id,
                            epoch: msg.epoch,
                            message_hash: proof.message_hash,
                            proof: proof.proof,
                        });
                        break;
                    } else {
                        return Err(anyhow!(
                            "Signing completed but no aggregation proof provided"
                        ));
                    }
                }
                SigningStatus::Failed => {
                    return Err(anyhow!(
                        "Signing failed for request_id: {}",
                        msg.request_id
                    ));
                }
                SigningStatus::Timeout => {
                    return Err(anyhow!(
                        "Signing timed out for request_id: {}",
                        msg.request_id
                    ));
                }
                SigningStatus::Pending => {
                    debug!("Signing pending, waiting for more updates...");
                }
                SigningStatus::Unspecified => {
                    warn!("Received unspecified signing status");
                }
            }
        }

        result.ok_or_else(|| anyhow!("Stream ended without completion"))
    }

    /// Sign a message with a specific required epoch
    pub async fn sign_message_wait_with_epoch(
        &self,
        key_tag: u8,
        message: &[u8],
        required_epoch: u64,
    ) -> Result<SignWithProofResult> {
        let mut client = self.connect().await?;

        let request = SignMessageWaitRequest {
            key_tag: key_tag as u32,
            message: message.to_vec(),
            required_epoch: Some(required_epoch),
        };

        debug!(
            "Sending SignMessageWait request with key_tag={}, required_epoch={}",
            key_tag, required_epoch
        );

        let response = client.sign_message_wait(request).await?;
        let mut stream = response.into_inner();

        let mut result: Option<SignWithProofResult> = None;

        while let Some(msg) = stream.next().await {
            let msg = msg?;

            let status = SigningStatus::try_from(msg.status)
                .unwrap_or(SigningStatus::Unspecified);

            match status {
                SigningStatus::Completed => {
                    if let Some(proof) = msg.aggregation_proof {
                        info!(
                            "Signing completed! request_id: {}, epoch: {}",
                            msg.request_id, msg.epoch
                        );
                        result = Some(SignWithProofResult {
                            request_id: msg.request_id,
                            epoch: msg.epoch,
                            message_hash: proof.message_hash,
                            proof: proof.proof,
                        });
                        break;
                    } else {
                        return Err(anyhow!(
                            "Signing completed but no aggregation proof provided"
                        ));
                    }
                }
                SigningStatus::Failed => {
                    return Err(anyhow!(
                        "Signing failed for request_id: {}",
                        msg.request_id
                    ));
                }
                SigningStatus::Timeout => {
                    return Err(anyhow!(
                        "Signing timed out for request_id: {}",
                        msg.request_id
                    ));
                }
                SigningStatus::Pending => {
                    debug!("Signing pending, waiting for more updates...");
                }
                SigningStatus::Unspecified => {
                    warn!("Received unspecified signing status");
                }
            }
        }

        result.ok_or_else(|| anyhow!("Stream ended without completion"))
    }

    /// Get current epoch from the relay
    pub async fn get_current_epoch(&self) -> Result<u64> {
        let mut client = self.connect().await?;

        let request = GetCurrentEpochRequest {};
        let response = client.get_current_epoch(request).await?;

        Ok(response.into_inner().epoch)
    }

    /// Check if sidecar is healthy by trying to get current epoch
    pub async fn is_healthy(&self) -> bool {
        match self.get_current_epoch().await {
            Ok(epoch) => {
                debug!("Sidecar healthy, current epoch: {}", epoch);
                true
            }
            Err(e) => {
                warn!("Sidecar health check failed: {}", e);
                false
            }
        }
    }
}

// Keep these types for backward compatibility with main.rs
#[derive(Debug, Clone)]
pub struct SignResult {
    pub request_id: String,
    pub epoch: u64,
}

#[derive(Debug, Clone)]
pub struct AggregationProof {
    pub message_hash: Vec<u8>,
    pub proof: Vec<u8>,
}

impl From<SignWithProofResult> for (SignResult, AggregationProof) {
    fn from(r: SignWithProofResult) -> Self {
        (
            SignResult {
                request_id: r.request_id,
                epoch: r.epoch,
            },
            AggregationProof {
                message_hash: r.message_hash,
                proof: r.proof,
            },
        )
    }
}
