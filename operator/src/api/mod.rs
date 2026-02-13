use std::sync::Arc;
use std::time::Instant;

use alloy::primitives::B256;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::error::ApiError;
use crate::provider::DynProvider;
use crate::storage::Storage;
use crate::webhook::WebhookEvent;

mod security;
mod webhooks;

pub use security::{cors_middleware, security_middleware, SecurityState};
pub use webhooks::handle_oz_relayer_webhook;

/// Application state shared across handlers
#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub storage: Arc<Storage>,
    pub provider: DynProvider,
    pub config: Arc<AppConfig>,
    pub start_time: Instant,
}

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub uptime_seconds: u64,
    pub version: &'static str,
}

/// Webhook response (Fix #12)
#[derive(Serialize)]
struct WebhookResponse {
    status: &'static str,
    message: &'static str,
    event_type: String,
}

/// Pagination query parameters (Fix #13)
#[derive(Debug, Deserialize)]
struct PaginationParams {
    limit: Option<usize>,
    offset: Option<usize>,
    /// Filter by message status (pending, processing, signed). Default: all
    status: Option<String>,
}

/// Submission status summary for API response
#[derive(Serialize)]
struct SubmissionStatusSummary {
    state: crate::storage::SubmissionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relayer_tx_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// Message with processing and submission status for debug API
#[derive(Serialize)]
struct MessageWithStatus {
    #[serde(flatten)]
    message: crate::storage::MessageData,
    /// Internal processing status: Pending, Processing, Signed
    status: crate::storage::MessageStatus,
    /// On-chain submission status (if submitted)
    #[serde(skip_serializing_if = "Option::is_none")]
    submission: Option<SubmissionStatusSummary>,
}

/// Messages list response with pagination
#[derive(Serialize)]
struct MessagesResponse {
    messages: Vec<MessageWithStatus>,
    count: usize,
    limit: usize,
    offset: usize,
}

/// Create the main API router
pub fn create_router(state: AppState) -> Router {
    let router = Router::new()
        // Health endpoint
        .route("/healthz", get(health_check))
        // Provider webhook endpoint
        .route("/webhook/events", post(handle_webhook))
        // OZ Relayer webhook endpoint
        .route(
            "/api/v1/webhooks/oz-relayer",
            post(handle_oz_relayer_webhook),
        )
        // Debug endpoints
        .route("/debug/v1/messages", get(list_messages))
        .route("/debug/v1/messages/:message_id", get(get_message))
        .route("/debug/v1/pending", get(list_pending));

    // Register provider-specific routes (matches Go's IProvider.RegisterAPIHandlers pattern)
    let router = state.provider.register_api_routes(router);

    router.layer(TraceLayer::new_for_http()).with_state(state)
}

/// Health check handler
async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        uptime_seconds: state.start_time.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Webhook event handler (Fix #12: Returns JSON response instead of empty 200)
async fn handle_webhook(
    State(state): State<AppState>,
    Json(event): Json<WebhookEvent>,
) -> Result<Json<WebhookResponse>, AppError> {
    // Extract event type from matched events or use monitor name
    let event_type = event
        .evm
        .matched_on_args
        .events
        .first()
        .map(|e| e.signature.clone())
        .unwrap_or_else(|| event.evm.monitor.name.clone());
    state.provider.handle_webhook_event(&event).await?;
    Ok(Json(WebhookResponse {
        status: "success",
        message: "Event received and processed",
        event_type,
    }))
}

/// List messages with pagination
/// Returns ALL messages with their processing status and submission status
async fn list_messages(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<MessagesResponse>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    // List all messages with their status
    let all_messages = state.storage.list_all_messages_with_status()?;

    // Parse status filter
    let target_status = params.status.as_deref().and_then(|s| match s {
        "pending" => Some(crate::storage::MessageStatus::Pending),
        "processing" => Some(crate::storage::MessageStatus::Processing),
        "signed" => Some(crate::storage::MessageStatus::Signed),
        _ => None,
    });

    // Filter by status if specified
    let filtered: Vec<_> = match target_status {
        Some(status) => all_messages.into_iter().filter(|(_, s)| *s == status).collect(),
        None => all_messages,
    };

    // Apply pagination and fetch submission status for each message
    let messages: Vec<_> = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(msg, status)| {
            // Look up submission status for this message
            let submission = state
                .storage
                .get_submission_status(msg.metadata.destination_chain, &msg.metadata.message_id)
                .ok()
                .flatten()
                .map(|sub| SubmissionStatusSummary {
                    state: sub.status,
                    tx_hash: sub.tx_hash.map(|h| h.to_string()),
                    relayer_tx_id: sub.relayer_tx_id,
                    last_error: sub.last_error,
                });

            MessageWithStatus {
                message: msg,
                status,
                submission,
            }
        })
        .collect();

    Ok(Json(MessagesResponse {
        count: messages.len(),
        messages,
        limit,
        offset,
    }))
}

/// Get single message by ID (Fix #11)
async fn get_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
) -> Result<Json<crate::storage::MessageData>, AppError> {
    let id = message_id
        .parse::<B256>()
        .map_err(|_| ApiError::BadRequest("invalid message ID format".into()))?;
    let msg = state
        .storage
        .get_message(&id)?
        .ok_or_else(|| ApiError::NotFound("message not found".into()))?;
    Ok(Json(msg))
}

/// List pending merkle roots (debug endpoint)
async fn list_pending(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    let pending = state.storage.list_pending_merkle_roots()?;
    let roots: Vec<_> = pending.keys().map(|r| r.to_string()).collect();
    Ok(Json(roots))
}

/// Application error type for API responses
pub struct AppError(ApiError);

impl From<ApiError> for AppError {
    fn from(err: ApiError) -> Self {
        AppError(err)
    }
}

impl From<crate::error::ProviderError> for AppError {
    fn from(err: crate::error::ProviderError) -> Self {
        AppError(ApiError::Provider(err))
    }
}

impl From<crate::error::StorageError> for AppError {
    fn from(err: crate::error::StorageError) -> Self {
        AppError(ApiError::Storage(err))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let error_msg = self.0.to_string();
        let status: StatusCode = self.0.into();
        let body = Json(serde_json::json!({
            "error": error_msg
        }));
        (status, body).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::error::{ApiError, ProviderError, StorageError};
    use crate::provider::Provider;
    use crate::storage::{MessageData, MessageMetadata, MessageStatus, SubmissionStatus};
    use crate::config::{
        AppConfig, DatabaseConfig, LayerZeroConfig, LoggingConfig, OzRelayerConfig, SecurityConfig,
        ServerConfig, SignerConfig, SymbioticRelayConfig,
    };
    use alloy::primitives::B256;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use tempfile::tempdir;

    struct TestProvider {
        seen: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl Provider for TestProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn handle_webhook_event(
            &self,
            _event: &crate::webhook::WebhookEvent,
        ) -> Result<(), ProviderError> {
            let mut seen = self.seen.lock().unwrap();
            *seen += 1;
            Ok(())
        }
    }

    fn test_storage_arc() -> (Arc<Storage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = Storage::new(&path).unwrap();
        (Arc::new(storage), dir)
    }

    fn test_config_arc() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 3000,
                read_timeout: std::time::Duration::from_secs(30),
                write_timeout: std::time::Duration::from_secs(30),
                idle_timeout: std::time::Duration::from_secs(120),
                security: SecurityConfig::default(),
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
                timeout: std::time::Duration::from_secs(30),
                retry_backoff: std::time::Duration::from_secs(1),
            },
            signer: SignerConfig {
                event_poll_interval: std::time::Duration::from_secs(15),
                sign_job_interval: std::time::Duration::from_secs(1),
                sign_worker_count: 2,
                min_batch_size: 1,
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![31338],
            provider: "layerzero".to_string(),
            layerzero: Some(LayerZeroConfig {
                eid_to_chain_id: {
                    let mut map = HashMap::new();
                    map.insert(40232, 31338);
                    map
                },
                target_addresses: {
                    let mut map = HashMap::new();
                    map.insert(31338, "0x1234567890123456789012345678901234567890".to_string());
                    map
                },
            }),
            chainlink_ccv: None,
        })
    }

    fn b256_from_seed(seed: u8) -> B256 {
        B256::from_slice(&[seed; 32])
    }

    fn test_merkle_tree(
        root_hash: B256,
        message_ids: Vec<B256>,
        source_chain: u64,
        destination_chain: u64,
    ) -> crate::storage::MerkleTreeData {
        let leaf_hashes = message_ids
            .iter()
            .map(|id| B256::from_slice(&alloy::primitives::keccak256(id.as_slice()).0))
            .collect();

        crate::storage::MerkleTreeData {
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

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "ok",
            uptime_seconds: 123,
            version: "0.1.0",
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"uptime_seconds\":123"));
        assert!(json.contains("\"version\":\"0.1.0\""));
    }

    #[test]
    fn test_app_error_from_api_error() {
        let api_err = ApiError::NotFound("message not found".into());
        let app_err = AppError::from(api_err);
        // Just verify it compiles and converts
        let response = app_err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_app_error_from_provider_error() {
        let provider_err = ProviderError::UnknownEvent("test".into());
        let app_err = AppError::from(provider_err);
        let response = app_err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_app_error_from_storage_error() {
        let storage_err = StorageError::NotFound("key".into());
        let app_err = AppError::from(storage_err);
        let response = app_err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_api_error_bad_request_status() {
        let err = ApiError::BadRequest("invalid input".into());
        let status: StatusCode = err.into();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_api_error_not_found_status() {
        let err = ApiError::NotFound("resource not found".into());
        let status: StatusCode = err.into();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_api_error_internal_status() {
        let err = ApiError::Internal("internal error".into());
        let status: StatusCode = err.into();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_pagination_params_defaults() {
        // Test deserialization with defaults
        let json = "{}";
        let params: PaginationParams = serde_json::from_str(json).unwrap();
        assert!(params.limit.is_none());
        assert!(params.offset.is_none());
        assert!(params.status.is_none());
    }

    #[test]
    fn test_pagination_params_custom_values() {
        let json = r#"{"limit": 10, "offset": 5, "status": "pending"}"#;
        let params: PaginationParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.limit, Some(10));
        assert_eq!(params.offset, Some(5));
        assert_eq!(params.status, Some("pending".to_string()));
    }

    #[test]
    fn test_webhook_response_serialization() {
        let response = WebhookResponse {
            status: "success",
            message: "Event received",
            event_type: "JobAssigned".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"success\""));
        assert!(json.contains("\"event_type\":\"JobAssigned\""));
    }

    #[test]
    fn test_messages_response_serialization() {
        let response = MessagesResponse {
            messages: vec![],
            count: 0,
            limit: 50,
            offset: 0,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"count\":0"));
        assert!(json.contains("\"limit\":50"));
        assert!(json.contains("\"offset\":0"));
    }

    #[test]
    fn test_submission_status_summary_serialization() {
        let summary = SubmissionStatusSummary {
            state: crate::storage::SubmissionState::Pending,
            tx_hash: None,
            relayer_tx_id: Some("tx-123".to_string()),
            last_error: None,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"state\":\"Pending\""));
        assert!(json.contains("\"relayer_tx_id\":\"tx-123\""));
        // tx_hash should be omitted when None
        assert!(!json.contains("tx_hash"));
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (storage, _dir) = test_storage_arc();
        let provider: DynProvider = Arc::new(TestProvider {
            seen: Arc::new(Mutex::new(0)),
        });
        let config = test_config_arc();
        let state = AppState {
            storage,
            provider,
            config,
            start_time: Instant::now(),
        };
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_messages_and_filter() {
        let (storage, _dir) = test_storage_arc();
        let provider: DynProvider = Arc::new(TestProvider {
            seen: Arc::new(Mutex::new(0)),
        });
        let config = test_config_arc();

        let msg_id = b256_from_seed(1);
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 100,
                message_id: msg_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: b"test".to_vec(),
        };
        storage.save_message(&msg).unwrap();
        storage.update_message_status(&msg_id, MessageStatus::Processing).unwrap();

        let mut sub = SubmissionStatus::new_pending(msg_id, B256::ZERO, 31338);
        sub.set_relayer_tx_id("tx-1".to_string());
        storage.save_submission_status(&sub).unwrap();

        let state = AppState {
            storage,
            provider,
            config,
            start_time: Instant::now(),
        };
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/debug/v1/messages?status=processing&limit=10&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_message_routes() {
        let (storage, _dir) = test_storage_arc();
        let provider: DynProvider = Arc::new(TestProvider {
            seen: Arc::new(Mutex::new(0)),
        });
        let config = test_config_arc();

        let msg_id = b256_from_seed(2);
        let msg = MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 100,
                message_id: msg_id,
                event_tx_hash: B256::ZERO,
                ttl: None,
            },
            data: b"test".to_vec(),
        };
        storage.save_message(&msg).unwrap();

        let state = AppState {
            storage,
            provider,
            config,
            start_time: Instant::now(),
        };
        let app = create_router(state);

        let response = app.clone()
            .oneshot(
                Request::builder()
                    .uri(&format!("/debug/v1/messages/{msg_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app.clone()
            .oneshot(
                Request::builder()
                    .uri("/debug/v1/messages/not-a-b256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let missing = b256_from_seed(9);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/debug/v1/messages/{missing}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_pending() {
        let (storage, _dir) = test_storage_arc();
        let provider: DynProvider = Arc::new(TestProvider {
            seen: Arc::new(Mutex::new(0)),
        });
        let config = test_config_arc();

        let root = b256_from_seed(3);
        let tree = test_merkle_tree(
            root,
            vec![b256_from_seed(4)],
            1,
            31338,
        );
        storage.save_merkle_tree(&tree).unwrap();

        let state = AppState {
            storage,
            provider,
            config,
            start_time: Instant::now(),
        };
        let app = create_router(state);

        let response = app
            .oneshot(Request::builder().uri("/debug/v1/pending").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_handle_webhook_event_type() {
        let (storage, _dir) = test_storage_arc();
        let seen = Arc::new(Mutex::new(0));
        let provider: DynProvider = Arc::new(TestProvider { seen: seen.clone() });
        let config = test_config_arc();
        let state = AppState {
            storage,
            provider,
            config,
            start_time: Instant::now(),
        };
        let app = create_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "EVM": {
                "logs": [],
                "matched_on_args": { "events": [] },
                "monitor": { "name": "Test Monitor" },
                "network_slug": "test"
            }
        }))
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/events")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*seen.lock().unwrap(), 1);
    }
}
