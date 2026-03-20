//! Webhook handlers for external service notifications

use alloy::primitives::B256;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::AppState;
use crate::storage::{SubmissionState, SubmissionStatus};

type HmacSha256 = Hmac<Sha256>;

/// OZ Relayer webhook event payload
#[derive(Debug, Deserialize)]
pub struct OzRelayerWebhook {
    /// Event ID
    pub id: String,
    /// Event type (e.g., "transaction_update")
    pub event: String,
    /// Event timestamp (required for deserialization, not used after parsing)
    #[allow(dead_code)]
    pub timestamp: String,
    /// Event payload
    pub payload: WebhookPayload,
}

/// Webhook payload for transaction updates
#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    /// OZ Relayer transaction ID
    pub id: String,
    /// Transaction status
    pub status: String,
    /// Transaction hash (once on-chain)
    pub hash: Option<String>,
    /// Reason for status (especially for failures)
    #[serde(rename = "statusReason")]
    pub status_reason: Option<String>,
}

/// Response for webhook endpoint
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub status: &'static str,
    pub message: &'static str,
}

/// Handle OZ Relayer webhook notifications
///
/// POST /api/v1/webhooks/oz-relayer
pub async fn handle_oz_relayer_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let secret = match state.config.server.security.oz_relayer_webhook_secret.as_deref() {
        Some(secret) if !secret.is_empty() => secret,
        _ => {
            tracing::error!(
                "OZ Relayer webhook endpoint accessed but OZ_RELAYER_WEBHOOK_SECRET not configured"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(WebhookResponse {
                    status: "error",
                    message: "Webhook secret unavailable",
                }),
            );
        }
    };

    if let Err(e) = verify_webhook_signature(&headers, &body, secret) {
        tracing::warn!(error = %e, "webhook signature verification failed");
        return (
            StatusCode::UNAUTHORIZED,
            Json(WebhookResponse {
                status: "error",
                message: "Invalid signature",
            }),
        );
    }

    // Parse webhook payload
    let webhook: OzRelayerWebhook = match serde_json::from_str(&body) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse webhook payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(WebhookResponse {
                    status: "error",
                    message: "Invalid payload",
                }),
            );
        }
    };

    tracing::debug!(
        event_id = %webhook.id,
        event_type = %webhook.event,
        tx_id = %webhook.payload.id,
        status = %webhook.payload.status,
        "received OZ Relayer webhook"
    );

    // Only process transaction_update events
    if webhook.event != "transaction_update" {
        return (
            StatusCode::OK,
            Json(WebhookResponse {
                status: "ok",
                message: "Event type ignored",
            }),
        );
    }

    // Look up submission by relayer tx ID
    match state
        .storage
        .get_submission_by_relayer_tx_id(&webhook.payload.id)
    {
        Ok(Some(mut status)) => {
            // Update status based on webhook
            update_status_from_webhook(&mut status, &webhook.payload);

            if let Err(e) = state.storage.save_submission_status(&status) {
                tracing::error!(error = %e, "failed to save submission status");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(WebhookResponse {
                        status: "error",
                        message: "Failed to update status",
                    }),
                );
            }

            tracing::info!(
                message_id = %status.message_id,
                relayer_tx_id = %webhook.payload.id,
                new_status = %webhook.payload.status,
                "submission status updated via webhook"
            );

            (
                StatusCode::OK,
                Json(WebhookResponse {
                    status: "ok",
                    message: "Status updated",
                }),
            )
        }
        Ok(None) => {
            tracing::debug!(
                relayer_tx_id = %webhook.payload.id,
                "webhook received for unknown transaction"
            );
            (
                StatusCode::OK,
                Json(WebhookResponse {
                    status: "ok",
                    message: "Transaction not found",
                }),
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to look up submission");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WebhookResponse {
                    status: "error",
                    message: "Database error",
                }),
            )
        }
    }
}

/// Verify HMAC-SHA256 signature from X-Signature header
///
/// OZ Relayer sends the signature as Base64-encoded HMAC-SHA256 of the JSON body.
fn verify_webhook_signature(
    headers: &HeaderMap,
    body: &str,
    secret: &str,
) -> Result<(), &'static str> {
    let signature = headers
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .ok_or("Missing X-Signature header")?;

    // OZ Relayer sends Base64-encoded signature
    let expected_sig = STANDARD
        .decode(signature)
        .map_err(|_| "Invalid signature format")?;

    // Compute HMAC
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| "Invalid secret")?;
    mac.update(body.as_bytes());
    let computed = mac.finalize().into_bytes();

    // Constant-time comparison
    if computed[..].ct_eq(&expected_sig).into() {
        Ok(())
    } else {
        Err("Signature mismatch")
    }
}

/// Update submission status from webhook payload
fn update_status_from_webhook(status: &mut SubmissionStatus, payload: &WebhookPayload) {
    match payload.status.to_lowercase().as_str() {
        "confirmed" | "mined" => {
            let tx_hash = payload
                .hash
                .as_ref()
                .and_then(|h| h.strip_prefix("0x").unwrap_or(h).parse::<B256>().ok());
            status.mark_confirmed(tx_hash);
        }
        "failed" | "canceled" | "expired" => {
            status.mark_failed();
            if let Some(ref reason) = payload.status_reason {
                status.last_error = Some(reason.clone());
            }
        }
        "sent" | "submitted" => {
            status.status = SubmissionState::Submitted;
        }
        _ => {
            // Unknown status, log but don't change
            tracing::debug!(status = %payload.status, "unknown webhook status");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use axum::http::HeaderValue;

    #[test]
    fn test_verify_webhook_signature_valid() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;

        // Compute expected signature (Base64-encoded, matching OZ Relayer format)
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Signature", HeaderValue::from_str(&signature).unwrap());

        assert!(verify_webhook_signature(&headers, body, secret).is_ok());
    }

    #[test]
    fn test_verify_webhook_signature_invalid_base64() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;

        // Invalid Base64 string
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Signature",
            HeaderValue::from_static("not-valid-base64!!!"),
        );

        let result = verify_webhook_signature(&headers, body, secret);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Invalid signature format");
    }

    #[test]
    fn test_verify_webhook_signature_wrong_signature() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;

        // Valid Base64 but wrong HMAC
        let wrong_sig = STANDARD.encode(b"wrong signature bytes here!!");

        let mut headers = HeaderMap::new();
        headers.insert("X-Signature", HeaderValue::from_str(&wrong_sig).unwrap());

        let result = verify_webhook_signature(&headers, body, secret);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Signature mismatch");
    }

    #[test]
    fn test_verify_webhook_signature_missing() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;
        let headers = HeaderMap::new();

        let result = verify_webhook_signature(&headers, body, secret);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing X-Signature header");
    }

    #[test]
    fn test_verify_webhook_signature_hex_rejected() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;

        // Compute signature as hex (old format that should now be rejected)
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let hex_signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Signature",
            HeaderValue::from_str(&hex_signature).unwrap(),
        );

        // Hex-encoded signatures should fail (not valid Base64 for HMAC bytes)
        let result = verify_webhook_signature(&headers, body, secret);
        // Hex string may partially decode as Base64 but won't match the HMAC
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_webhook_signature_different_body() {
        let secret = "test-secret";
        let body = r#"{"id":"123","event":"test"}"#;
        let tampered_body = r#"{"id":"456","event":"test"}"#;

        // Compute signature for original body
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Signature", HeaderValue::from_str(&signature).unwrap());

        // Verification should fail with tampered body
        let result = verify_webhook_signature(&headers, tampered_body, secret);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Signature mismatch");
    }

    #[test]
    fn test_verify_webhook_signature_different_secret() {
        let secret = "test-secret";
        let wrong_secret = "wrong-secret";
        let body = r#"{"id":"123","event":"test"}"#;

        // Compute signature with correct secret
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Signature", HeaderValue::from_str(&signature).unwrap());

        // Verification should fail with wrong secret
        let result = verify_webhook_signature(&headers, body, wrong_secret);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Signature mismatch");
    }

    #[test]
    fn test_parse_oz_relayer_webhook() {
        // Real webhook payload format from OZ Relayer
        let payload = r#"{
            "id": "event-uuid-123",
            "event": "transaction_update",
            "timestamp": "2026-01-26T22:15:38.082163380+00:00",
            "payload": {
                "id": "tx-uuid-456",
                "status": "confirmed",
                "hash": "0x1234567890abcdef",
                "statusReason": null
            }
        }"#;

        let webhook: OzRelayerWebhook = serde_json::from_str(payload).unwrap();

        assert_eq!(webhook.id, "event-uuid-123");
        assert_eq!(webhook.event, "transaction_update");
        assert_eq!(webhook.payload.id, "tx-uuid-456");
        assert_eq!(webhook.payload.status, "confirmed");
        assert_eq!(webhook.payload.hash, Some("0x1234567890abcdef".to_string()));
        assert!(webhook.payload.status_reason.is_none());
    }

    #[test]
    fn test_parse_webhook_with_status_reason() {
        let payload = r#"{
            "id": "event-uuid-123",
            "event": "transaction_update",
            "timestamp": "2026-01-26T22:15:38.000Z",
            "payload": {
                "id": "tx-uuid-456",
                "status": "failed",
                "hash": null,
                "statusReason": "execution reverted: AlreadyVerified"
            }
        }"#;

        let webhook: OzRelayerWebhook = serde_json::from_str(payload).unwrap();

        assert_eq!(webhook.payload.status, "failed");
        assert!(webhook.payload.hash.is_none());
        assert_eq!(
            webhook.payload.status_reason,
            Some("execution reverted: AlreadyVerified".to_string())
        );
    }

    fn test_submission_status() -> SubmissionStatus {
        SubmissionStatus::new_pending(B256::ZERO, B256::ZERO, 31338)
    }

    #[test]
    fn test_update_status_from_webhook_confirmed() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "confirmed".to_string(),
            hash: Some(
                "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ),
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Confirmed);
        assert!(status.tx_hash.is_some());
    }

    #[test]
    fn test_update_status_from_webhook_mined() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "mined".to_string(),
            hash: Some(
                "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ),
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Confirmed);
    }

    #[test]
    fn test_update_status_from_webhook_failed() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "failed".to_string(),
            hash: None,
            status_reason: Some("execution reverted".to_string()),
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Failed);
        assert_eq!(status.last_error, Some("execution reverted".to_string()));
    }

    #[test]
    fn test_update_status_from_webhook_submitted() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "submitted".to_string(),
            hash: Some("0xabcdef".to_string()),
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Submitted);
    }

    #[test]
    fn test_update_status_case_insensitive() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "CONFIRMED".to_string(),
            hash: Some(
                "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ),
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Confirmed);
    }

    #[test]
    fn test_update_status_sent() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "sent".to_string(),
            hash: None,
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Submitted);
    }

    #[test]
    fn test_update_status_unknown() {
        let mut status = test_submission_status();
        let original_status = status.status;

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "some_unknown_status".to_string(),
            hash: None,
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        // Status should not change for unknown
        assert_eq!(status.status, original_status);
    }

    #[test]
    fn test_update_status_canceled() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "canceled".to_string(),
            hash: None,
            status_reason: Some("user requested".to_string()),
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Failed);
        assert_eq!(status.last_error, Some("user requested".to_string()));
    }

    #[test]
    fn test_update_status_expired() {
        let mut status = test_submission_status();

        let payload = WebhookPayload {
            id: "tx-123".to_string(),
            status: "expired".to_string(),
            hash: None,
            status_reason: None,
        };

        update_status_from_webhook(&mut status, &payload);

        assert_eq!(status.status, SubmissionState::Failed);
    }

    #[test]
    fn test_oz_relayer_webhook_deserialization_minimal() {
        let json = r#"{
            "id": "evt-1",
            "event": "transaction_update",
            "timestamp": "2026-01-27T00:00:00Z",
            "payload": {
                "id": "tx-1",
                "status": "pending"
            }
        }"#;

        let webhook: OzRelayerWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(webhook.id, "evt-1");
        assert_eq!(webhook.event, "transaction_update");
        assert_eq!(webhook.payload.id, "tx-1");
        assert_eq!(webhook.payload.status, "pending");
        assert!(webhook.payload.hash.is_none());
        assert!(webhook.payload.status_reason.is_none());
    }

    #[test]
    fn test_webhook_response_serialization() {
        let response = WebhookResponse {
            status: "ok",
            message: "processed",
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"message\":\"processed\""));
    }
}
