use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::storage::{MessageData, MessageMetadata};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcceptanceHookConfig {
    Native {
        name: String,
    },
    Webhook {
        #[serde(default)]
        name: Option<String>,
        url: String,
        secret: String,
        #[serde(with = "humantime_serde", default = "default_webhook_timeout")]
        timeout: Duration,
        #[serde(
            rename = "errorBackoff",
            with = "humantime_serde",
            default = "default_error_backoff"
        )]
        error_backoff: Duration,
        #[serde(rename = "maxAttempts", default = "default_max_attempts")]
        max_attempts: u32,
    },
}

impl AcceptanceHookConfig {
    pub fn key(&self) -> String {
        match self {
            Self::Native { name } => format!("native:{name}"),
            Self::Webhook { name, url, .. } => {
                format!("webhook:{}", name.as_deref().unwrap_or(url))
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Native { name } => {
                if name != "provider" {
                    return Err(format!(
                        "unsupported native acceptance hook '{name}' (supported: provider)"
                    ));
                }
            }
            Self::Webhook {
                name,
                url,
                secret,
                timeout,
                error_backoff,
                max_attempts,
            } => {
                if name.as_deref().is_some_and(str::is_empty) {
                    return Err("webhook acceptance hook name cannot be empty".to_string());
                }
                let parsed_url = url::Url::parse(url)
                    .map_err(|e| format!("invalid webhook acceptance hook URL '{url}': {e}"))?;
                if !matches!(parsed_url.scheme(), "http" | "https") {
                    return Err(format!(
                        "webhook acceptance hook URL '{url}' must use http or https"
                    ));
                }
                if secret.is_empty() {
                    return Err("webhook acceptance hook secret cannot be empty".to_string());
                }
                if timeout.is_zero() {
                    return Err(
                        "webhook acceptance hook timeout must be greater than 0".to_string()
                    );
                }
                if error_backoff.is_zero() {
                    return Err(
                        "webhook acceptance hook errorBackoff must be greater than 0".to_string(),
                    );
                }
                if *max_attempts == 0 {
                    return Err(
                        "webhook acceptance hook maxAttempts must be greater than 0".to_string()
                    );
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceContext {
    pub defer_count: u32,
    pub previous_defer_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceDecision {
    Accept,
    Reject {
        reason: Option<String>,
    },
    Defer {
        until: DateTime<Utc>,
        reason: Option<String>,
    },
}

impl AcceptanceDecision {
    pub fn accept() -> Self {
        Self::Accept
    }

    pub fn reject(reason: impl Into<Option<String>>) -> Self {
        Self::Reject {
            reason: reason.into(),
        }
    }

    pub fn defer(until: DateTime<Utc>, reason: impl Into<Option<String>>) -> Self {
        Self::Defer {
            until,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookHookError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("webhook returned non-success status {0}")]
    Status(StatusCode),

    #[error("malformed JSON response: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid webhook decision response: {0}")]
    InvalidResponse(String),

    #[error("invalid webhook signature secret")]
    InvalidSecret,
}

#[derive(Debug, Serialize)]
struct WebhookRequest<'a> {
    message: WebhookMessage<'a>,
    context: &'a AcceptanceContext,
}

#[derive(Debug, Serialize)]
struct WebhookMessage<'a> {
    metadata: &'a MessageMetadata,
    data: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WebhookDecisionKind {
    Accept,
    Reject,
    Defer,
}

#[derive(Debug, Deserialize)]
struct WebhookDecisionResponse {
    decision: WebhookDecisionKind,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    until: Option<DateTime<Utc>>,
}

impl WebhookDecisionResponse {
    fn into_decision(self) -> Result<AcceptanceDecision, WebhookHookError> {
        match self.decision {
            WebhookDecisionKind::Accept => Ok(AcceptanceDecision::Accept),
            WebhookDecisionKind::Reject => Ok(AcceptanceDecision::Reject {
                reason: self.reason,
            }),
            WebhookDecisionKind::Defer => {
                let until = self.until.ok_or_else(|| {
                    WebhookHookError::InvalidResponse(
                        "defer decision requires an RFC3339 until".to_string(),
                    )
                })?;
                Ok(AcceptanceDecision::Defer {
                    until,
                    reason: self.reason,
                })
            }
        }
    }
}

pub async fn evaluate_webhook(
    client: &reqwest::Client,
    url: &str,
    secret: &str,
    timeout: Duration,
    message: &MessageData,
    context: &AcceptanceContext,
) -> Result<AcceptanceDecision, WebhookHookError> {
    let body = build_webhook_body(message, context)?;
    let signature = sign_body(secret, &body)?;

    let response = client
        .post(url)
        .timeout(timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-Hook-Signature", signature)
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(WebhookHookError::Status(response.status()));
    }

    let bytes = response.bytes().await?;
    let decoded: WebhookDecisionResponse = serde_json::from_slice(&bytes)?;
    decoded.into_decision()
}

fn build_webhook_body(
    message: &MessageData,
    context: &AcceptanceContext,
) -> Result<Vec<u8>, WebhookHookError> {
    let request = WebhookRequest {
        message: WebhookMessage {
            metadata: &message.metadata,
            data: BASE64.encode(&message.data),
        },
        context,
    };
    serde_json::to_vec(&request).map_err(WebhookHookError::Json)
}

fn sign_body(secret: &str, body: &[u8]) -> Result<String, WebhookHookError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| WebhookHookError::InvalidSecret)?;
    mac.update(body);
    Ok(format!(
        "sha256={}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

fn default_webhook_timeout() -> Duration {
    Duration::from_secs(5)
}

fn default_error_backoff() -> Duration {
    Duration::from_secs(30)
}

fn default_max_attempts() -> u32 {
    3
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_message() -> MessageData {
        MessageData {
            metadata: MessageMetadata {
                source_chain: 1,
                destination_chain: 31338,
                block_number: 12345,
                message_id: B256::from_slice(&[0x11; 32]),
                event_tx_hash: B256::from_slice(&[0x22; 32]),
                ttl: None,
            },
            data: b"hello".to_vec(),
        }
    }

    #[test]
    fn webhook_body_uses_base64_payload_and_context() {
        let context = AcceptanceContext {
            defer_count: 2,
            previous_defer_reason: Some("awaiting approval".to_string()),
        };
        let body = build_webhook_body(&test_message(), &context).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["message"]["data"], "aGVsbG8=");
        assert_eq!(value["context"]["defer_count"], 2);
        assert_eq!(
            value["context"]["previous_defer_reason"],
            "awaiting approval"
        );
    }

    #[test]
    fn webhook_signature_is_sha256_hmac() {
        let sig = sign_body("secret", br#"{"ok":true}"#).unwrap();
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), "sha256=".len() + 64);
    }

    #[test]
    fn webhook_defer_requires_until() {
        let response: WebhookDecisionResponse =
            serde_json::from_str(r#"{"decision":"defer"}"#).unwrap();

        let err = response.into_decision().unwrap_err();
        assert!(err.to_string().contains("until"));
    }

    #[test]
    fn webhook_reject_allows_null_reason() {
        let response: WebhookDecisionResponse =
            serde_json::from_str(r#"{"decision":"reject","reason":null}"#).unwrap();

        assert_eq!(
            response.into_decision().unwrap(),
            AcceptanceDecision::Reject { reason: None }
        );
    }

    #[test]
    fn hook_config_validates_native_name() {
        let hook = AcceptanceHookConfig::Native {
            name: "unknown".to_string(),
        };

        assert!(hook.validate().unwrap_err().contains("unsupported"));
    }

    #[test]
    fn hook_config_rejects_non_http_webhook_url() {
        let hook = AcceptanceHookConfig::Webhook {
            name: Some("approval".to_string()),
            url: "file:///tmp/hook".to_string(),
            secret: "shared-secret".to_string(),
            timeout: Duration::from_secs(5),
            error_backoff: Duration::from_secs(30),
            max_attempts: 3,
        };

        assert!(hook.validate().unwrap_err().contains("http or https"));
    }

    #[tokio::test]
    async fn evaluate_webhook_sends_signed_request() {
        let message = test_message();
        let context = AcceptanceContext {
            defer_count: 0,
            previous_defer_reason: None,
        };
        let body = build_webhook_body(&message, &context).unwrap();
        let signature = sign_body("shared-secret", &body).unwrap();
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(header("X-Hook-Signature", signature.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "accept"
            })))
            .mount(&server)
            .await;

        let decision = evaluate_webhook(
            &reqwest::Client::new(),
            &server.uri(),
            "shared-secret",
            Duration::from_secs(5),
            &message,
            &context,
        )
        .await
        .unwrap();

        assert_eq!(decision, AcceptanceDecision::Accept);
    }

    #[tokio::test]
    async fn evaluate_webhook_treats_non_success_as_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let err = evaluate_webhook(
            &reqwest::Client::new(),
            &server.uri(),
            "shared-secret",
            Duration::from_secs(5),
            &test_message(),
            &AcceptanceContext {
                defer_count: 0,
                previous_defer_reason: None,
            },
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("503"));
    }
}
