use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request, StatusCode};
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

/// Verify API key
pub fn verify_api_key(provided: &str, expected: &str) -> Result<(), SecurityError> {
    if provided.is_empty() {
        return Err(SecurityError::MissingApiKey);
    }
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err(SecurityError::InvalidApiKey);
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

    // Verify API key if configured
    if let Some(expected_key) = &security.config.api_key
        && !expected_key.is_empty()
    {
        let provided_key = req
            .headers()
            .get("X-API-Key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if let Err(e) = verify_api_key(provided_key, expected_key) {
            tracing::warn!(error = %e, path, "API key verification failed");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    // Verify HMAC for webhook endpoints
    if path == "/webhook/events"
        && let Some(secret) = &security.config.webhook_secret
        && !secret.is_empty()
    {
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

        if let Err(e) = verify_hmac(&bytes, &timestamp, &signature, secret) {
            tracing::warn!(error = %e, "webhook HMAC verification failed");
            return Err(StatusCode::UNAUTHORIZED);
        }

        if let Err(e) = verify_timestamp(&timestamp, security.config.timestamp_window) {
            tracing::warn!(error = %e, "webhook timestamp verification failed");
            return Err(StatusCode::UNAUTHORIZED);
        }

        // Reconstruct request with buffered body
        let req = Request::from_parts(parts, Body::from(bytes));
        return Ok(next.run(req).await);
    }

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
                (
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    "GET, POST, OPTIONS",
                ),
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
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "Content-Type, X-API-Key, X-Signature, X-Timestamp, X-Event-Type"
            .parse()
            .unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_api_key_verification() {
        assert!(verify_api_key("correct_key", "correct_key").is_ok());
        assert!(verify_api_key("wrong_key", "correct_key").is_err());
        assert!(verify_api_key("", "correct_key").is_err());
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
}
