use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::config::SecurityConfig;
use crate::error::SecurityError;

type HmacSha256 = Hmac<Sha256>;

/// Security state for middleware
#[derive(Clone)]
pub struct SecurityState {
    pub config: SecurityConfig,
}

/// Verify HMAC signature
pub fn verify_hmac(
    body: &[u8],
    timestamp: &str,
    signature: &str,
    secret: &str,
) -> Result<(), SecurityError> {
    if signature.is_empty() {
        return Err(SecurityError::MissingSignature);
    }
    if timestamp.is_empty() {
        return Err(SecurityError::MissingTimestamp);
    }

    // Reconstruct message: body + timestamp
    let mut message = body.to_vec();
    message.extend_from_slice(timestamp.as_bytes());

    // Calculate expected signature
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| SecurityError::InvalidSecret)?;
    mac.update(&message);
    let expected = hex::encode(mac.finalize().into_bytes());

    // Timing-safe comparison
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(SecurityError::InvalidSignature);
    }

    Ok(())
}

/// Timing-safe string comparison
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Verify timestamp is within acceptable window
pub fn verify_timestamp(timestamp_str: &str, max_age: Duration) -> Result<(), SecurityError> {
    let timestamp_ms: i64 = timestamp_str
        .parse()
        .map_err(|_| SecurityError::InvalidTimestamp)?;

    let request_time =
        DateTime::from_timestamp_millis(timestamp_ms).ok_or(SecurityError::InvalidTimestamp)?;

    let now = Utc::now();
    let diff = (now - request_time).abs();

    if diff > chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::seconds(300)) {
        return Err(SecurityError::TimestampExpired);
    }

    Ok(())
}

/// Security middleware
pub async fn security_middleware(
    State(security): State<SecurityState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();

    // Skip security for health endpoints
    if path == "/healthz" {
        return Ok(next.run(req).await);
    }

    // Block debug endpoints if disabled (security hardening for production)
    if path.starts_with("/debug/") && !security.config.enable_debug_endpoints {
        tracing::warn!(path, "debug endpoint access denied (disabled in config)");
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify HMAC for /webhook/events endpoint
    if path == "/webhook/events" {
        // Defense in depth: reject if secret is not configured
        // (startup validation should catch this, but be safe)
        let secret = match &security.config.webhook_secret {
            Some(s) if !s.is_empty() => s,
            _ => {
                tracing::error!("webhook endpoint accessed but WEBHOOK_SECRET not configured");
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        };

        return verify_webhook_request(req, next, secret, security.config.timestamp_window).await;
    }

    Ok(next.run(req).await)
}

/// Verify webhook request with HMAC signature and timestamp
async fn verify_webhook_request(
    req: Request<Body>,
    next: Next,
    secret: &str,
    timestamp_window: std::time::Duration,
) -> Result<Response, StatusCode> {
    // Extract headers before moving req
    let signature = req
        .headers()
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let timestamp = req
        .headers()
        .get("X-Timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Read body for verification (need to buffer)
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    // Use generic "Unauthorized" for all auth failures (don't leak details)
    if let Err(e) = verify_hmac(&bytes, &timestamp, &signature, secret) {
        tracing::warn!(error = %e, "webhook authentication failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    if let Err(e) = verify_timestamp(&timestamp, timestamp_window) {
        tracing::warn!(error = %e, "webhook authentication failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Reconstruct request with buffered body
    let req = Request::from_parts(parts, Body::from(bytes));
    Ok(next.run(req).await)
}

/// CORS middleware - adds CORS headers when enabled in config
pub async fn cors_middleware(
    State(security): State<SecurityState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Only apply CORS if enabled
    if !security.config.enable_cors {
        return next.run(req).await;
    }

    // Handle preflight OPTIONS request
    if req.method() == Method::OPTIONS {
        return (
            StatusCode::OK,
            [
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"),
                (
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    "Content-Type, X-API-Key, X-Signature, X-Timestamp, X-Event-Type",
                ),
            ],
        )
            .into_response();
    }

    // For non-preflight requests, run the handler then add CORS headers
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        "*".parse().expect("valid static header"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS".parse().expect("valid static header"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "Content-Type, X-API-Key, X-Signature, X-Timestamp, X-Event-Type"
            .parse()
            .expect("valid static header"),
    );
    response
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use std::time::Duration as StdDuration;
    use tower::ServiceExt;

    #[test]
    fn test_hmac_verification() {
        let body = b"test body";
        let timestamp = "1234567890";
        let secret = "test_secret";

        // Calculate expected signature
        let mut message = body.to_vec();
        message.extend_from_slice(timestamp.as_bytes());

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&message);
        let signature = hex::encode(mac.finalize().into_bytes());

        // Should pass
        assert!(verify_hmac(body, timestamp, &signature, secret).is_ok());

        // Should fail with wrong signature
        assert!(verify_hmac(body, timestamp, "wrong_signature", secret).is_err());

        // Should fail with empty signature
        assert!(verify_hmac(body, timestamp, "", secret).is_err());

        // Should fail with empty timestamp
        assert!(verify_hmac(body, "", &signature, secret).is_err());
    }

    #[test]
    fn test_timestamp_verification() {
        let max_age = Duration::from_secs(300); // 5 minutes

        // Valid timestamp (now)
        let now_ms = Utc::now().timestamp_millis();
        assert!(
            verify_timestamp(&now_ms.to_string(), max_age).is_ok(),
            "current timestamp should be valid"
        );

        // Old timestamp (10 minutes ago)
        let old_ms = Utc::now().timestamp_millis() - 600_000;
        assert!(
            verify_timestamp(&old_ms.to_string(), max_age).is_err(),
            "old timestamp should be invalid"
        );

        // Future timestamp (10 minutes ahead)
        let future_ms = Utc::now().timestamp_millis() + 600_000;
        assert!(
            verify_timestamp(&future_ms.to_string(), max_age).is_err(),
            "future timestamp should be invalid"
        );

        // Empty timestamp
        assert!(
            verify_timestamp("", max_age).is_err(),
            "empty timestamp should be invalid"
        );

        // Invalid format
        assert!(
            verify_timestamp("not-a-number", max_age).is_err(),
            "invalid format should be rejected"
        );
    }

    // ============ Phase 3: Additional Security Tests ============

    #[test]
    fn test_hmac_missing_signature() {
        let body = b"test body";
        let timestamp = "1234567890";
        let secret = "test_secret";

        let result = verify_hmac(body, timestamp, "", secret);
        assert!(matches!(
            result,
            Err(crate::error::SecurityError::MissingSignature)
        ));
    }

    #[test]
    fn test_hmac_missing_timestamp() {
        let body = b"test body";
        let signature = "some_signature";
        let secret = "test_secret";

        let result = verify_hmac(body, "", signature, secret);
        assert!(matches!(
            result,
            Err(crate::error::SecurityError::MissingTimestamp)
        ));
    }

    #[test]
    fn test_verify_timestamp_overflow_protection() {
        let max_age = Duration::from_secs(300);

        // Very large timestamp - should still be rejected (future)
        let huge_ts = "9999999999999";
        assert!(verify_timestamp(huge_ts, max_age).is_err());
    }

    #[test]
    fn test_verify_timestamp_boundary() {
        let max_age = Duration::from_secs(300);

        // Exactly at boundary (290 seconds ago should pass)
        let boundary_ms = Utc::now().timestamp_millis() - 290_000;
        assert!(
            verify_timestamp(&boundary_ms.to_string(), max_age).is_ok(),
            "timestamp just within window should be valid"
        );

        // Just past boundary (310 seconds ago should fail)
        let past_boundary_ms = Utc::now().timestamp_millis() - 310_000;
        assert!(
            verify_timestamp(&past_boundary_ms.to_string(), max_age).is_err(),
            "timestamp just past window should be invalid"
        );
    }

    #[test]
    fn test_constant_time_eq_equal() {
        let a = b"hello world";
        let b = b"hello world";
        assert!(constant_time_eq(a, b));
    }

    #[test]
    fn test_constant_time_eq_different_content() {
        let a = b"hello world";
        let b = b"hello worle";
        assert!(!constant_time_eq(a, b));
    }

    #[test]
    fn test_constant_time_eq_different_length() {
        let a = b"hello";
        let b = b"hello world";
        assert!(!constant_time_eq(a, b));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        let a: &[u8] = b"";
        let b: &[u8] = b"";
        assert!(constant_time_eq(a, b));
    }

    #[test]
    fn test_security_state_clone() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: Some("test".to_string()),
                oz_relayer_webhook_secret: Some("test2".to_string()),
                timestamp_window: Duration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: true,
            },
        };
        let cloned = state.clone();
        assert_eq!(state.config.webhook_secret, cloned.config.webhook_secret);
    }

    #[test]
    fn test_verify_hmac_different_body() {
        let body1 = b"original body";
        let body2 = b"modified body";
        let timestamp = "1234567890";
        let secret = "test_secret";

        // Calculate signature for body1
        let mut message = body1.to_vec();
        message.extend_from_slice(timestamp.as_bytes());
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&message);
        let signature = hex::encode(mac.finalize().into_bytes());

        // Should fail with body2
        let result = verify_hmac(body2, timestamp, &signature, secret);
        assert!(matches!(
            result,
            Err(crate::error::SecurityError::InvalidSignature)
        ));
    }

    #[test]
    fn test_verify_hmac_different_timestamp() {
        let body = b"test body";
        let timestamp1 = "1234567890";
        let timestamp2 = "1234567891";
        let secret = "test_secret";

        // Calculate signature for timestamp1
        let mut message = body.to_vec();
        message.extend_from_slice(timestamp1.as_bytes());
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&message);
        let signature = hex::encode(mac.finalize().into_bytes());

        // Should fail with timestamp2
        let result = verify_hmac(body, timestamp2, &signature, secret);
        assert!(matches!(
            result,
            Err(crate::error::SecurityError::InvalidSignature)
        ));
    }

    #[test]
    fn test_verify_hmac_different_secret() {
        let body = b"test body";
        let timestamp = "1234567890";
        let secret1 = "secret_one";
        let secret2 = "secret_two";

        // Calculate signature with secret1
        let mut message = body.to_vec();
        message.extend_from_slice(timestamp.as_bytes());
        let mut mac = HmacSha256::new_from_slice(secret1.as_bytes()).unwrap();
        mac.update(&message);
        let signature = hex::encode(mac.finalize().into_bytes());

        // Should fail with secret2
        let result = verify_hmac(body, timestamp, &signature, secret2);
        assert!(matches!(
            result,
            Err(crate::error::SecurityError::InvalidSignature)
        ));
    }

    #[tokio::test]
    async fn test_security_middleware_health_bypass() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: None,
                oz_relayer_webhook_secret: None,
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: false,
            },
        };

        let app = Router::new()
            .route("/healthz", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                security_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_security_middleware_debug_blocked() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: Some("a".repeat(32)),
                oz_relayer_webhook_secret: Some("b".repeat(32)),
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: false,
            },
        };

        let app = Router::new()
            .route("/debug/v1/messages", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                security_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/debug/v1/messages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_security_middleware_webhook_auth() {
        let secret = "test_secret_key_1234567890123456789012";
        let timestamp = Utc::now().timestamp_millis().to_string();
        let body = b"{}";

        let mut message = body.to_vec();
        message.extend_from_slice(timestamp.as_bytes());
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&message);
        let signature = hex::encode(mac.finalize().into_bytes());

        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: Some(secret.to_string()),
                oz_relayer_webhook_secret: Some("b".repeat(32)),
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: true,
            },
        };

        let app = Router::new()
            .route("/webhook/events", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                security_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/events")
                    .header("X-Signature", signature)
                    .header("X-Timestamp", timestamp)
                    .body(Body::from(body.as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_security_middleware_webhook_missing_secret() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: None,
                oz_relayer_webhook_secret: Some("b".repeat(32)),
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: true,
            },
        };

        let app = Router::new()
            .route("/webhook/events", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                security_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/events")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_security_middleware_oz_relayer_webhook_passthrough() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: Some("a".repeat(32)),
                oz_relayer_webhook_secret: Some("b".repeat(32)),
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: false,
                enable_debug_endpoints: true,
            },
        };

        let app = Router::new()
            .route("/api/v1/webhooks/oz-relayer", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                security_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/oz-relayer")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_cors_middleware_options() {
        let state = SecurityState {
            config: SecurityConfig {
                webhook_secret: Some("a".repeat(32)),
                oz_relayer_webhook_secret: Some("b".repeat(32)),
                timestamp_window: StdDuration::from_secs(300),
                enable_cors: true,
                enable_debug_endpoints: true,
            },
        };

        let app = Router::new()
            .route("/healthz", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(state, cors_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
