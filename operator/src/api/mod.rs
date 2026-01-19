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
