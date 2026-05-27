use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::acceptance::AcceptanceHookConfig;
use crate::error::ConfigError;
pub use crate::provider::types::{ChainlinkCcvConfig, LayerZeroConfig};

// ── Environment config structs (mirrors config/environments/*.json) ──────────

/// Top-level environment configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConfig {
    pub version: u32,
    pub name: String,
    pub active_provider: String,
    pub chains: ChainsConfig,
    #[serde(default)]
    pub relay: Option<RelayTimingConfig>,
    #[serde(default)]
    pub operator: Option<OperatorSettings>,
}

/// Source and destination chain configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ChainsConfig {
    pub source: ChainConfig,
    pub destination: ChainConfig,
}

/// Per-chain configuration with immutable chain metadata and predeploys.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub eid: u32,
    #[serde(default)]
    pub confirmations: u64,
    #[serde(default)]
    pub predeploys: serde_json::Value,
}

/// Deployment addresses loaded from config/deployments/<env>.json.
#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentsConfig {
    pub source: serde_json::Value,
    pub destination: serde_json::Value,
}

#[derive(Debug, Clone, Copy)]
enum ChainRole {
    Source,
    Destination,
}

impl ChainRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Destination => "destination",
        }
    }
}

/// Relay timing parameters.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayTimingConfig {
    pub epoch_duration_seconds: Option<u64>,
    pub slashing_window_seconds: Option<u64>,
    pub epoch_start_delay_seconds: Option<u64>,
}

/// Operator-specific settings from environment JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorSettings {
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub event_poll_interval: Option<String>,
    #[serde(default)]
    pub sign_job_interval: Option<String>,
    #[serde(default)]
    pub sign_worker_count: Option<usize>,
    #[serde(default)]
    pub min_batch_size: Option<u64>,
    #[serde(default)]
    pub acceptance_hooks: Vec<AcceptanceHookConfig>,
    #[serde(default)]
    pub enable_debug_endpoints: bool,
}

impl EnvironmentConfig {
    /// Load an environment config from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Validation(format!(
                "failed to read environment config {}: {}",
                path.display(),
                e
            ))
        })?;
        serde_json::from_str(&content).map_err(|e| {
            ConfigError::Validation(format!(
                "failed to parse environment config {}: {}",
                path.display(),
                e
            ))
        })
    }
}

impl DeploymentsConfig {
    /// Load deployments from a JSON file with shape `{source:{...}, destination:{...}}`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Validation(format!(
                "failed to read deployments config {}: {}",
                path.display(),
                e
            ))
        })?;
        serde_json::from_str(&content).map_err(|e| {
            ConfigError::Validation(format!(
                "failed to parse deployments config {}: {}",
                path.display(),
                e
            ))
        })
    }

    fn chain(&self, role: ChainRole) -> &serde_json::Value {
        match role {
            ChainRole::Source => &self.source,
            ChainRole::Destination => &self.destination,
        }
    }

    /// Get a deployment address from the deployment config.
    fn deployment(
        &self,
        role: ChainRole,
        chain: &ChainConfig,
        key: &str,
    ) -> Result<String, ConfigError> {
        self.chain(role)
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                ConfigError::Validation(format!(
                    "missing deployment '{}' for {} chain '{}'",
                    key,
                    role.as_str(),
                    chain.name
                ))
            })
    }

    /// Get a nested deployment address (e.g. chainlinkCcv.ccv).
    fn nested_deployment(
        &self,
        role: ChainRole,
        chain: &ChainConfig,
        parent: &str,
        key: &str,
    ) -> Result<String, ConfigError> {
        self.chain(role)
            .get(parent)
            .and_then(|value| value.get(key))
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                ConfigError::Validation(format!(
                    "missing deployment '{}.{}' for {} chain '{}'",
                    parent,
                    key,
                    role.as_str(),
                    chain.name
                ))
            })
    }
}

/// Main application configuration
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub logging: LoggingConfig,
    pub symbiotic_relay: SymbioticRelayConfig,
    pub signer: SignerConfig,
    #[serde(default)]
    pub oz_relayer: OzRelayerConfig,
    /// Destination chain IDs for validation
    #[serde(default)]
    pub destination_chains: Vec<u64>,
    pub provider: String,
    #[serde(default)]
    pub layerzero: Option<LayerZeroConfig>,
    #[serde(default)]
    pub chainlink_ccv: Option<ChainlinkCcvConfig>,
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(with = "humantime_serde", default = "default_read_timeout")]
    pub read_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_write_timeout")]
    pub write_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_idle_timeout")]
    pub idle_timeout: Duration,
    #[serde(default)]
    pub security: SecurityConfig,
}

/// Database configuration
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

/// Logging configuration
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

/// Symbiotic relay configuration (for BLS signature aggregation)
#[derive(Debug, Clone, Deserialize)]
pub struct SymbioticRelayConfig {
    pub address: String,
    /// BLS key identifier (0-127) - specifies which relay key to use for signing
    #[serde(default = "default_key_tag")]
    pub key_tag: u8,
    #[serde(default)]
    pub use_mock: bool,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_retry_backoff")]
    pub retry_backoff: Duration,
}

/// Signer job configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SignerConfig {
    #[serde(with = "humantime_serde", default = "default_event_poll_interval")]
    pub event_poll_interval: Duration,
    #[serde(with = "humantime_serde", default = "default_sign_job_interval")]
    pub sign_job_interval: Duration,
    #[serde(default = "default_sign_worker_count")]
    pub sign_worker_count: usize,
    #[serde(default = "default_min_batch_size")]
    pub min_batch_size: u64,
    #[serde(default)]
    pub acceptance_hooks: Vec<AcceptanceHookConfig>,
}

/// OpenZeppelin Relayer configuration
/// Note: OZ_RELAYER_API_KEY is read from env var for relayer authentication.
#[derive(Debug, Clone, Deserialize)]
pub struct OzRelayerConfig {
    /// Base URL for OZ Relayer API (e.g., "http://oz-relayer:8080")
    pub base_url: String,
    /// Poll interval for finding new proofs to submit
    #[serde(with = "humantime_serde", default = "default_oz_poll_interval")]
    pub poll_interval: Duration,
    /// Fallback status polling interval (for missed webhooks)
    #[serde(with = "humantime_serde", default = "default_oz_status_poll_interval")]
    pub status_poll_interval: Duration,
    /// Default gas speed tier
    #[serde(default = "default_oz_speed")]
    pub default_speed: String,
    /// HTTP request timeout
    #[serde(with = "humantime_serde", default = "default_oz_timeout")]
    pub timeout: Duration,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(with = "humantime_serde", default = "default_retry_backoff")]
    pub retry_backoff: Duration,
    /// Chain to relayer ID mappings
    #[serde(default)]
    pub chain_relayers: Vec<ChainRelayerEntry>,
}

/// Entry mapping a chain to its OZ Relayer instance
#[derive(Debug, Clone, Deserialize)]
pub struct ChainRelayerEntry {
    /// EVM chain ID
    pub chain_id: u64,
    /// OZ Relayer ID for this chain
    pub relayer_id: String,
    /// Target contract address on this chain for transaction submission.
    pub target_address: String,
}

impl Default for OzRelayerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8080".to_string(),
            poll_interval: default_oz_poll_interval(),
            status_poll_interval: default_oz_status_poll_interval(),
            default_speed: default_oz_speed(),
            timeout: default_oz_timeout(),
            max_retries: default_max_retries(),
            retry_backoff: default_retry_backoff(),
            chain_relayers: Vec::new(),
        }
    }
}

/// Minimum length for webhook secrets (32 chars = 256 bits)
pub const MIN_SECRET_LENGTH: usize = 32;

/// Security configuration
///
/// # Required Environment Variables
///
/// - `WEBHOOK_SECRET`: HMAC secret for webhook signature verification (min 32 chars)
/// - `OZ_RELAYER_WEBHOOK_SECRET`: Secret for OZ Relayer webhook auth (min 32 chars)
///
/// Generate secrets with: `openssl rand -hex 32`
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    /// HMAC secret for webhook signature verification (required, min 32 chars)
    pub webhook_secret: Option<String>,
    /// HMAC secret for OZ Relayer webhook verification (required, min 32 chars)
    pub oz_relayer_webhook_secret: Option<String>,
    /// Maximum age for request timestamps
    #[serde(with = "humantime_serde", default = "default_timestamp_window")]
    pub timestamp_window: Duration,
    #[serde(default)]
    pub enable_cors: bool,
    /// Enable debug endpoints (/debug/v1/*). Disable in production.
    #[serde(default = "default_enable_debug_endpoints")]
    pub enable_debug_endpoints: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            webhook_secret: None,
            oz_relayer_webhook_secret: None,
            timestamp_window: default_timestamp_window(),
            enable_cors: false,
            enable_debug_endpoints: default_enable_debug_endpoints(),
        }
    }
}

impl SecurityConfig {
    /// Validate security configuration for production readiness.
    /// Returns an error if required secrets are missing or too weak.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate webhook_secret
        match &self.webhook_secret {
            None => {
                return Err(ConfigError::Validation(
                    "WEBHOOK_SECRET environment variable is required. Generate with: openssl rand -hex 32".to_string(),
                ));
            }
            Some(secret) if secret.len() < MIN_SECRET_LENGTH => {
                return Err(ConfigError::Validation(format!(
                    "WEBHOOK_SECRET must be at least {} characters (got {}). Generate with: openssl rand -hex 32",
                    MIN_SECRET_LENGTH,
                    secret.len()
                )));
            }
            Some(_) => {}
        }

        // Validate oz_relayer_webhook_secret
        match &self.oz_relayer_webhook_secret {
            None => {
                return Err(ConfigError::Validation(
                    "OZ_RELAYER_WEBHOOK_SECRET environment variable is required. Generate with: openssl rand -hex 32".to_string(),
                ));
            }
            Some(secret) if secret.len() < MIN_SECRET_LENGTH => {
                return Err(ConfigError::Validation(format!(
                    "OZ_RELAYER_WEBHOOK_SECRET must be at least {} characters (got {}). Generate with: openssl rand -hex 32",
                    MIN_SECRET_LENGTH,
                    secret.len()
                )));
            }
            Some(_) => {}
        }

        if self.timestamp_window.is_zero() {
            return Err(ConfigError::Validation(
                "security.timestamp_window must be greater than 0".to_string(),
            ));
        }

        // Warn about debug endpoints (not a hard error, just logged)
        if self.enable_debug_endpoints {
            tracing::warn!(
                "debug endpoints are enabled - disable in production with enable_debug_endpoints: false"
            );
        }

        Ok(())
    }
}

fn default_enable_debug_endpoints() -> bool {
    false
}

// Default value functions
fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_read_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_write_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_idle_timeout() -> Duration {
    Duration::from_secs(120)
}

fn default_db_path() -> String {
    "./data/redb".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

fn default_key_tag() -> u8 {
    15
}

fn default_max_retries() -> u32 {
    3
}

fn default_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_retry_backoff() -> Duration {
    Duration::from_secs(1)
}

fn default_event_poll_interval() -> Duration {
    Duration::from_secs(15)
}

fn default_sign_job_interval() -> Duration {
    Duration::from_secs(1)
}

fn default_sign_worker_count() -> usize {
    5
}

fn default_min_batch_size() -> u64 {
    1
}

fn default_oz_poll_interval() -> Duration {
    Duration::from_secs(5)
}

fn default_oz_status_poll_interval() -> Duration {
    Duration::from_secs(30)
}

fn default_oz_speed() -> String {
    "fast".to_string()
}

fn default_oz_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_timestamp_window() -> Duration {
    Duration::from_secs(300)
}

/// Parse a simple duration string like "30s", "2s", "5m".
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        secs.parse::<u64>().ok().map(Duration::from_secs)
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse::<u64>().ok().map(Duration::from_secs)
    }
}

impl AppConfig {
    /// Load config from explicit environment and deployments JSON files.
    pub fn load_from_paths(
        environment_path: impl AsRef<Path>,
        deployments_path: impl AsRef<Path>,
        sidecar_address: &str,
        relayer_id: &str,
    ) -> Result<Self, ConfigError> {
        let environment = EnvironmentConfig::load(environment_path)?;
        let deployments = DeploymentsConfig::load(deployments_path)?;

        Self::from_environment(&environment, &deployments, sidecar_address, relayer_id)
    }

    /// Build an AppConfig from environment metadata and deployment addresses.
    pub fn from_environment(
        env: &EnvironmentConfig,
        deployments: &DeploymentsConfig,
        sidecar_address: &str,
        relayer_id: &str,
    ) -> Result<Self, ConfigError> {
        let src = &env.chains.source;
        let dst = &env.chains.destination;

        // Parse operator settings from environment JSON
        let op = env.operator.as_ref();

        let event_poll_interval = op
            .and_then(|o| o.event_poll_interval.as_deref())
            .and_then(parse_duration)
            .unwrap_or_else(default_event_poll_interval);

        let sign_job_interval = op
            .and_then(|o| o.sign_job_interval.as_deref())
            .and_then(parse_duration)
            .unwrap_or_else(default_sign_job_interval);

        let sign_worker_count = op
            .and_then(|o| o.sign_worker_count)
            .unwrap_or_else(default_sign_worker_count);

        let min_batch_size = op
            .and_then(|o| o.min_batch_size)
            .unwrap_or_else(default_min_batch_size);
        let mut acceptance_hooks = op.map(|o| o.acceptance_hooks.clone()).unwrap_or_default();
        for hook in &mut acceptance_hooks {
            hook.resolve_env().map_err(ConfigError::Validation)?;
        }

        let log_level = op
            .and_then(|o| o.log_level.clone())
            .unwrap_or_else(default_log_level);

        // Build provider-specific config
        let provider = env.active_provider.clone();
        let (layerzero, chainlink_ccv) = match provider.as_str() {
            "layerzero" => {
                let dst_dvn = deployments.deployment(ChainRole::Destination, dst, "dvn")?;
                let mut eid_to_chain_id = HashMap::new();
                eid_to_chain_id.insert(src.eid, src.chain_id);
                eid_to_chain_id.insert(dst.eid, dst.chain_id);

                let mut target_addresses = HashMap::new();
                target_addresses.insert(dst.chain_id, dst_dvn);

                (
                    Some(LayerZeroConfig {
                        eid_to_chain_id,
                        target_addresses,
                    }),
                    None,
                )
            }
            "chainlink_ccv" => {
                let src_ccv =
                    deployments.nested_deployment(ChainRole::Source, src, "chainlinkCcv", "ccv")?;
                let dst_ccv = deployments.nested_deployment(
                    ChainRole::Destination,
                    dst,
                    "chainlinkCcv",
                    "ccv",
                )?;
                let src_onramp = deployments.nested_deployment(
                    ChainRole::Source,
                    src,
                    "chainlinkCcv",
                    "onRamp",
                )?;
                let dst_offramp = deployments.nested_deployment(
                    ChainRole::Destination,
                    dst,
                    "chainlinkCcv",
                    "offRamp",
                )?;

                (
                    None,
                    Some(ChainlinkCcvConfig {
                        source_chain_id: src.chain_id,
                        destination_chain_id: dst.chain_id,
                        source_chain_selector: src.chain_id, // TODO: separate selector field
                        destination_chain_selector: dst.chain_id,
                        source_ccv_address: src_ccv,
                        destination_ccv_address: dst_ccv,
                        source_onramp_address: src_onramp,
                        destination_offramp_address: dst_offramp,
                    }),
                )
            }
            other => {
                return Err(ConfigError::Validation(format!(
                    "unsupported provider: {}",
                    other
                )));
            }
        };

        // Build chain_relayers for OZ Relayer
        let relayer_target = match provider.as_str() {
            "layerzero" => deployments
                .deployment(ChainRole::Destination, dst, "dvn")
                .unwrap_or_default(),
            "chainlink_ccv" => deployments
                .nested_deployment(ChainRole::Destination, dst, "chainlinkCcv", "offRamp")
                .unwrap_or_default(),
            _ => String::new(),
        };
        let chain_relayers = if relayer_target.is_empty() {
            Vec::new()
        } else {
            vec![ChainRelayerEntry {
                chain_id: dst.chain_id,
                relayer_id: relayer_id.to_string(),
                target_address: relayer_target,
            }]
        };

        let enable_debug_endpoints = op.map(|o| o.enable_debug_endpoints).unwrap_or(false);

        let config = AppConfig {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
                read_timeout: default_read_timeout(),
                write_timeout: default_write_timeout(),
                idle_timeout: default_idle_timeout(),
                security: SecurityConfig {
                    enable_debug_endpoints,
                    ..SecurityConfig::default()
                },
            },
            database: DatabaseConfig {
                path: format!("/app/data/{}/redb", provider),
            },
            logging: LoggingConfig {
                level: log_level,
                format: default_log_format(),
            },
            symbiotic_relay: SymbioticRelayConfig {
                address: sidecar_address.to_string(),
                key_tag: default_key_tag(),
                use_mock: false,
                max_retries: default_max_retries(),
                timeout: default_timeout(),
                retry_backoff: default_retry_backoff(),
            },
            signer: SignerConfig {
                event_poll_interval,
                sign_job_interval,
                sign_worker_count,
                min_batch_size,
                acceptance_hooks,
            },
            oz_relayer: OzRelayerConfig {
                base_url: "http://oz-relayer:8080".to_string(),
                chain_relayers,
                ..OzRelayerConfig::default()
            },
            destination_chains: vec![dst.chain_id],
            provider,
            layerzero,
            chainlink_ccv,
        };

        config.validate()?;
        Ok(config)
    }

    /// Load runtime secrets from environment variables into the in-memory config.
    pub fn load_security_secrets_from_env(&mut self) -> Result<(), ConfigError> {
        self.server.security.webhook_secret =
            Some(std::env::var("WEBHOOK_SECRET").map_err(|_| {
                ConfigError::Validation(
                    "WEBHOOK_SECRET environment variable is required".to_string(),
                )
            })?);
        self.server.security.oz_relayer_webhook_secret =
            Some(std::env::var("OZ_RELAYER_WEBHOOK_SECRET").map_err(|_| {
                ConfigError::Validation(
                    "OZ_RELAYER_WEBHOOK_SECRET environment variable is required".to_string(),
                )
            })?);

        Ok(())
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.port == 0 {
            return Err(ConfigError::Validation("invalid port: 0".to_string()));
        }

        if self.database.path.is_empty() {
            return Err(ConfigError::Validation(
                "database path cannot be empty".to_string(),
            ));
        }

        if self.symbiotic_relay.address.is_empty() {
            return Err(ConfigError::Validation(
                "symbiotic_relay address cannot be empty".to_string(),
            ));
        }

        if self.destination_chains.is_empty() {
            return Err(ConfigError::Validation(
                "at least one destination chain is required".to_string(),
            ));
        }

        let mut hook_keys = HashSet::new();
        for hook in &self.signer.acceptance_hooks {
            hook.validate().map_err(ConfigError::Validation)?;
            let key = hook.key();
            if !hook_keys.insert(key.clone()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate acceptance hook key '{key}'; set a unique webhook name"
                )));
            }
        }

        Ok(())
    }

    /// Check if a chain ID is a supported destination
    pub fn is_supported_destination(&self, chain_id: u64) -> bool {
        self.destination_chains.contains(&chain_id)
    }

    /// Get the server address as host:port
    pub fn server_address(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::unwrap_err_used)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn valid_secret() -> String {
        "a]".repeat(MIN_SECRET_LENGTH) // 32+ chars
    }

    fn short_secret() -> String {
        "short".to_string() // < 32 chars
    }

    #[test]
    fn test_security_config_valid() {
        let config = SecurityConfig {
            webhook_secret: Some(valid_secret()),
            oz_relayer_webhook_secret: Some(valid_secret()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_security_config_missing_webhook_secret() {
        let config = SecurityConfig {
            webhook_secret: None,
            oz_relayer_webhook_secret: Some(valid_secret()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("WEBHOOK_SECRET")));
    }

    #[test]
    fn test_security_config_missing_oz_relayer_secret() {
        let config = SecurityConfig {
            webhook_secret: Some(valid_secret()),
            oz_relayer_webhook_secret: None,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ConfigError::Validation(msg) if msg.contains("OZ_RELAYER_WEBHOOK_SECRET"))
        );
    }

    #[test]
    fn test_security_config_short_webhook_secret() {
        let config = SecurityConfig {
            webhook_secret: Some(short_secret()),
            oz_relayer_webhook_secret: Some(valid_secret()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("at least")));
    }

    #[test]
    fn test_security_config_short_oz_relayer_secret() {
        let config = SecurityConfig {
            webhook_secret: Some(valid_secret()),
            oz_relayer_webhook_secret: Some(short_secret()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("at least")));
    }

    #[test]
    fn test_min_secret_length_constant() {
        // Ensure minimum is at least 32 chars (256 bits)
        assert!(MIN_SECRET_LENGTH >= 32);
    }

    // ============ Phase 2: Additional Config Tests ============

    #[test]
    fn test_app_config_validate_port_zero() {
        let mut config = test_config();
        config.server.port = 0;

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), ConfigError::Validation(msg) if msg.contains("port"))
        );
    }

    #[test]
    fn test_app_config_validate_empty_db_path() {
        let mut config = test_config();
        config.database.path = String::new();

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), ConfigError::Validation(msg) if msg.contains("database"))
        );
    }

    #[test]
    fn test_app_config_validate_empty_relay_address() {
        let mut config = test_config();
        config.symbiotic_relay.address = String::new();

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), ConfigError::Validation(msg) if msg.contains("symbiotic_relay"))
        );
    }

    #[test]
    fn test_app_config_validate_empty_chains() {
        let mut config = test_config();
        config.destination_chains = vec![];

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), ConfigError::Validation(msg) if msg.contains("destination chain"))
        );
    }

    #[test]
    fn test_is_supported_destination_valid() {
        let config = test_config();
        assert!(config.is_supported_destination(42161));
        assert!(config.is_supported_destination(31338));
    }

    #[test]
    fn test_is_supported_destination_invalid() {
        let config = test_config();
        assert!(!config.is_supported_destination(99999));
        assert!(!config.is_supported_destination(0));
    }

    #[test]
    fn test_server_address_format() {
        let config = test_config();
        let addr = config.server_address();
        assert_eq!(addr, "0.0.0.0:3000");
    }

    #[test]
    fn test_oz_relayer_config_defaults() {
        let config = OzRelayerConfig::default();
        assert_eq!(config.base_url, "http://localhost:8080");
        assert_eq!(config.default_speed, "fast");
        assert!(config.chain_relayers.is_empty());
    }

    #[test]
    fn test_all_default_functions() {
        // Test that all default functions produce valid values
        assert_eq!(default_host(), "0.0.0.0");
        assert_eq!(default_port(), 3000);
        assert!(!default_db_path().is_empty());
        assert_eq!(default_log_level(), "info");
        assert_eq!(default_log_format(), "json");
        assert_eq!(default_key_tag(), 15);
        assert_eq!(default_max_retries(), 3);
        assert_eq!(default_sign_worker_count(), 5);
        assert_eq!(default_min_batch_size(), 1);
        assert_eq!(default_oz_speed(), "fast");
        assert!(!default_enable_debug_endpoints());
    }

    #[test]
    fn test_layerzero_config_default() {
        let config = LayerZeroConfig::default();
        assert!(config.eid_to_chain_id.is_empty());
        assert!(config.target_addresses.is_empty());
    }

    #[test]
    fn test_app_config_valid() {
        let config = test_config();
        assert!(config.validate().is_ok());
    }

    // ============ Environment Config Tests ============

    fn test_env_config_json() -> &'static str {
        r#"{
            "version": 1,
            "name": "test",
            "activeProvider": "layerzero",
            "chains": {
                "source": {
                    "name": "anvil",
                    "chainId": 31337,
                    "eid": 31337,
                    "confirmations": 1,
                    "blockTimeMs": 1000,
                    "predeploys": {}
                },
                "destination": {
                    "name": "anvil-settlement",
                    "chainId": 31338,
                    "eid": 31338,
                    "confirmations": 1,
                    "blockTimeMs": 1000,
                    "predeploys": {}
                }
            },
            "operator": {
                "logLevel": "debug",
                "eventPollInterval": "30s",
                "signJobInterval": "2s",
                "signWorkerCount": 2,
                "minBatchSize": 1
            }
        }"#
    }

    fn test_env_config() -> EnvironmentConfig {
        serde_json::from_str(test_env_config_json()).unwrap()
    }

    fn test_deployments_config_json() -> &'static str {
        r#"{
            "source": {
                "dvn": "0x1111111111111111111111111111111111111111"
            },
            "destination": {
                "dvn": "0x3333333333333333333333333333333333333333",
                "relayInfra": {
                    "settlement": "0x5555555555555555555555555555555555555555",
                    "driver": "0x6666666666666666666666666666666666666666"
                }
            }
        }"#
    }

    fn test_deployments_config() -> DeploymentsConfig {
        serde_json::from_str(test_deployments_config_json()).unwrap()
    }

    fn write_temp_json_file(prefix: &str, contents: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{unique}.json"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn test_from_environment_layerzero() {
        let env = test_env_config();
        let deployments = test_deployments_config();
        let config =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap();

        assert_eq!(config.provider, "layerzero");
        assert_eq!(config.destination_chains, vec![31338]);
        assert_eq!(config.symbiotic_relay.address, "http://sidecar:8080");

        let lz = config.layerzero.unwrap();
        assert_eq!(lz.eid_to_chain_id.get(&31337), Some(&31337u64));
        assert_eq!(lz.eid_to_chain_id.get(&31338), Some(&31338u64));
        assert_eq!(
            lz.target_addresses.get(&31338),
            Some(&"0x3333333333333333333333333333333333333333".to_string())
        );
    }

    #[test]
    fn test_from_environment_sidecar_and_relayer() {
        let env = test_env_config();
        let deployments = test_deployments_config();
        let config =
            AppConfig::from_environment(&env, &deployments, "http://my-sidecar:8080", "my-relayer")
                .unwrap();

        assert_eq!(config.symbiotic_relay.address, "http://my-sidecar:8080");
        assert_eq!(config.oz_relayer.chain_relayers[0].relayer_id, "my-relayer");
    }

    fn test_ccv_env_config_json() -> &'static str {
        r#"{
            "version": 1,
            "name": "local-ccv",
            "activeProvider": "chainlink_ccv",
            "chains": {
                "source": {
                    "name": "anvil",
                    "chainId": 31337,
                    "eid": 31337,
                    "confirmations": 1,
                    "blockTimeMs": 1000,
                    "predeploys": {}
                },
                "destination": {
                    "name": "anvil-settlement",
                    "chainId": 31338,
                    "eid": 31338,
                    "confirmations": 1,
                    "blockTimeMs": 1000,
                    "predeploys": {}
                }
            },
            "operator": {
                "logLevel": "debug",
                "eventPollInterval": "30s",
                "signJobInterval": "2s",
                "signWorkerCount": 2,
                "minBatchSize": 1
            }
        }"#
    }

    fn test_ccv_deployments_config_json() -> &'static str {
        r#"{
            "source": {
                "chainlinkCcv": {
                    "ccv": "0x1111111111111111111111111111111111111111",
                    "onRamp": "0x2222222222222222222222222222222222222222",
                    "offRamp": "0x3333333333333333333333333333333333333333"
                }
            },
            "destination": {
                "chainlinkCcv": {
                    "ccv": "0x4444444444444444444444444444444444444444",
                    "onRamp": "0x5555555555555555555555555555555555555555",
                    "offRamp": "0x6666666666666666666666666666666666666666",
                    "settlement": "0x7777777777777777777777777777777777777777"
                },
                "relayInfra": {
                    "settlement": "0x7777777777777777777777777777777777777777",
                    "driver": "0x8888888888888888888888888888888888888888"
                }
            }
        }"#
    }

    #[test]
    fn test_from_environment_chainlink_ccv_uses_offramp_for_relayer_target() {
        let env: EnvironmentConfig = serde_json::from_str(test_ccv_env_config_json()).unwrap();
        let deployments: DeploymentsConfig =
            serde_json::from_str(test_ccv_deployments_config_json()).unwrap();

        let config =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap();

        assert_eq!(config.provider, "chainlink_ccv");
        assert_eq!(config.destination_chains, vec![31338]);
        assert_eq!(config.oz_relayer.chain_relayers.len(), 1);
        assert_eq!(
            config.oz_relayer.chain_relayers[0].target_address,
            "0x6666666666666666666666666666666666666666"
        );

        let ccv = config.chainlink_ccv.unwrap();
        assert_eq!(
            ccv.destination_offramp_address,
            "0x6666666666666666666666666666666666666666"
        );
    }

    #[test]
    fn test_from_environment_missing_deployment() {
        let env = test_env_config();
        let mut deployments = test_deployments_config();
        deployments.destination = serde_json::json!({});

        let result =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("dvn")));
    }

    #[test]
    fn test_from_environment_operator_settings() {
        let env = test_env_config();
        let deployments = test_deployments_config();
        let config =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap();

        assert_eq!(config.signer.event_poll_interval, Duration::from_secs(30));
        assert_eq!(config.signer.sign_job_interval, Duration::from_secs(2));
        assert_eq!(config.signer.sign_worker_count, 2);
        assert_eq!(config.signer.min_batch_size, 1);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_from_environment_acceptance_hooks() {
        let env_json = r#"{
            "version": 1,
            "name": "test",
            "activeProvider": "layerzero",
            "chains": {
                "source": {
                    "name": "anvil",
                    "chainId": 31337,
                    "eid": 31337,
                    "confirmations": 1,
                    "predeploys": {}
                },
                "destination": {
                    "name": "anvil-settlement",
                    "chainId": 31338,
                    "eid": 31338,
                    "confirmations": 1,
                    "predeploys": {}
                }
            },
            "operator": {
                "acceptanceHooks": [
                    { "type": "native", "name": "provider" },
                    {
                        "type": "webhook",
                        "name": "approval",
                        "url": "http://approval.local/hook",
                        "secret": "shared-secret",
                        "headers": {
                            "Authorization": "Bearer test-token",
                            "X-Approval-Scope": { "type": "plain", "value": "bridge" }
                        },
                        "timeout": "5s",
                        "errorBackoff": "30s",
                        "maxAttempts": 4
                    }
                ]
            }
        }"#;
        let env: EnvironmentConfig = serde_json::from_str(env_json).unwrap();
        let deployments = test_deployments_config();

        let config =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap();

        assert_eq!(config.signer.acceptance_hooks.len(), 2);
        let AcceptanceHookConfig::Webhook { headers, .. } = &config.signer.acceptance_hooks[1]
        else {
            panic!("expected webhook hook");
        };
        assert_eq!(
            headers.get("Authorization"),
            Some(&crate::acceptance::WebhookHeaderValue::Plain(
                "Bearer test-token".to_string()
            ))
        );
    }

    #[test]
    fn test_config_validate_rejects_unknown_native_acceptance_hook() {
        let env_json = r#"{
            "version": 1,
            "name": "test",
            "activeProvider": "layerzero",
            "chains": {
                "source": {
                    "name": "anvil",
                    "chainId": 31337,
                    "eid": 31337,
                    "confirmations": 1,
                    "predeploys": {}
                },
                "destination": {
                    "name": "anvil-settlement",
                    "chainId": 31338,
                    "eid": 31338,
                    "confirmations": 1,
                    "predeploys": {}
                }
            },
            "operator": {
                "acceptanceHooks": [
                    { "type": "native", "name": "unknown" }
                ]
            }
        }"#;
        let env: EnvironmentConfig = serde_json::from_str(env_json).unwrap();
        let deployments = test_deployments_config();

        let err =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported native acceptance hook")
        );
    }

    #[test]
    fn test_config_validate_rejects_duplicate_acceptance_hook_keys() {
        let env_json = r#"{
            "version": 1,
            "name": "test",
            "activeProvider": "layerzero",
            "chains": {
                "source": {
                    "name": "anvil",
                    "chainId": 31337,
                    "eid": 31337,
                    "confirmations": 1,
                    "predeploys": {}
                },
                "destination": {
                    "name": "anvil-settlement",
                    "chainId": 31338,
                    "eid": 31338,
                    "confirmations": 1,
                    "predeploys": {}
                }
            },
            "operator": {
                "acceptanceHooks": [
                    {
                        "type": "webhook",
                        "url": "http://approval.local/hook",
                        "secret": "first-secret"
                    },
                    {
                        "type": "webhook",
                        "url": "http://approval.local/hook",
                        "secret": "second-secret"
                    }
                ]
            }
        }"#;
        let env: EnvironmentConfig = serde_json::from_str(env_json).unwrap();
        let deployments = test_deployments_config();

        let err =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap_err();

        assert!(err.to_string().contains("duplicate acceptance hook key"));
    }

    #[test]
    fn test_load_from_paths_with_separate_environment_and_deployments_files() {
        let env_path = write_temp_json_file("operator-env", test_env_config_json());
        let deployments_path =
            write_temp_json_file("operator-deployments", test_deployments_config_json());

        let config = AppConfig::load_from_paths(
            &env_path,
            &deployments_path,
            "http://localhost:8081",
            "operator-relayer-1",
        )
        .unwrap();

        std::fs::remove_file(env_path).unwrap();
        std::fs::remove_file(deployments_path).unwrap();

        assert_eq!(config.symbiotic_relay.address, "http://localhost:8081");
        assert_eq!(
            config
                .layerzero
                .unwrap()
                .target_addresses
                .get(&31338)
                .cloned(),
            Some("0x3333333333333333333333333333333333333333".to_string())
        );
    }

    #[test]
    fn test_deployments_config_load_rejects_legacy_chains_wrapper() {
        let legacy_deployments = r#"{
            "chains": {
                "source": {},
                "destination": {}
            }
        }"#;
        let deployments_path =
            write_temp_json_file("operator-deployments-legacy", legacy_deployments);

        let result = DeploymentsConfig::load(&deployments_path);

        std::fs::remove_file(deployments_path).unwrap();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("missing field `source`")
        ));
    }

    #[test]
    fn test_security_config_zero_timestamp_window() {
        let config = SecurityConfig {
            webhook_secret: Some(valid_secret()),
            oz_relayer_webhook_secret: Some(valid_secret()),
            timestamp_window: Duration::from_secs(0),
            enable_cors: false,
            enable_debug_endpoints: false,
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("timestamp_window")));
    }

    #[test]
    fn test_from_environment_unsupported_provider() {
        let mut env = test_env_config();
        env.active_provider = "unknown_provider".to_string();
        let deployments = test_deployments_config();

        let result =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("unsupported provider")
        ));
    }

    #[test]
    fn test_from_environment_no_operator_settings_uses_defaults() {
        let mut env = test_env_config();
        env.operator = None;
        let deployments = test_deployments_config();

        let config =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer")
                .unwrap();

        assert_eq!(
            config.signer.event_poll_interval,
            default_event_poll_interval()
        );
        assert_eq!(config.signer.sign_job_interval, default_sign_job_interval());
        assert_eq!(config.signer.sign_worker_count, default_sign_worker_count());
        assert_eq!(config.signer.min_batch_size, default_min_batch_size());
        assert_eq!(config.logging.level, default_log_level());
    }

    #[test]
    fn test_parse_duration_edge_cases() {
        // Whitespace trimming
        assert_eq!(parse_duration("  30s  "), Some(Duration::from_secs(30)));
        // Empty string
        assert_eq!(parse_duration(""), None);
        // Minutes
        assert_eq!(parse_duration("1m"), Some(Duration::from_secs(60)));
        // Bare number (seconds)
        assert_eq!(parse_duration("10"), Some(Duration::from_secs(10)));
    }

    #[test]
    fn test_chain_role_as_str() {
        assert_eq!(ChainRole::Source.as_str(), "source");
        assert_eq!(ChainRole::Destination.as_str(), "destination");
    }

    #[test]
    fn test_deployments_deployment_missing_key() {
        let deployments = test_deployments_config();
        let chain = ChainConfig {
            name: "test".to_string(),
            chain_id: 31337,
            eid: 31337,
            confirmations: 1,
            predeploys: serde_json::json!({}),
        };

        let result = deployments.deployment(ChainRole::Source, &chain, "nonexistent_key");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent_key"));
    }

    #[test]
    fn test_deployments_nested_deployment_missing() {
        let deployments = test_deployments_config();
        let chain = ChainConfig {
            name: "test".to_string(),
            chain_id: 31337,
            eid: 31337,
            confirmations: 1,
            predeploys: serde_json::json!({}),
        };

        let result = deployments.nested_deployment(ChainRole::Source, &chain, "missing", "missing");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing.missing"));
    }

    #[test]
    fn test_environment_config_load_nonexistent() {
        let result = EnvironmentConfig::load("/tmp/nonexistent-path-abc123.json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to read"));
    }

    #[test]
    fn test_deployments_config_load_nonexistent() {
        let result = DeploymentsConfig::load("/tmp/nonexistent-path-abc123.json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to read"));
    }

    #[test]
    fn test_environment_config_load_invalid_json() {
        let path = write_temp_json_file("operator-env-bad", "not valid json");
        let result = EnvironmentConfig::load(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to parse"));
    }

    #[test]
    fn test_deployments_config_load_invalid_json() {
        let path = write_temp_json_file("operator-dep-bad", "not valid json");
        let result = DeploymentsConfig::load(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to parse"));
    }

    #[test]
    fn test_security_config_default_values() {
        let config = SecurityConfig::default();
        assert!(config.webhook_secret.is_none());
        assert!(config.oz_relayer_webhook_secret.is_none());
        assert!(!config.enable_cors);
        assert!(!config.enable_debug_endpoints);
        assert_eq!(config.timestamp_window, Duration::from_secs(300));
    }

    #[test]
    fn test_chainlink_ccv_missing_nested_deployment() {
        let mut env: EnvironmentConfig = serde_json::from_str(test_ccv_env_config_json()).unwrap();
        env.active_provider = "chainlink_ccv".to_string();

        // Missing chainlinkCcv.ccv in source
        let deployments: DeploymentsConfig = serde_json::from_str(
            r#"{
            "source": {},
            "destination": {
                "chainlinkCcv": {
                    "ccv": "0x4444444444444444444444444444444444444444",
                    "onRamp": "0x5555555555555555555555555555555555555555",
                    "offRamp": "0x6666666666666666666666666666666666666666"
                }
            }
        }"#,
        )
        .unwrap();

        let result =
            AppConfig::from_environment(&env, &deployments, "http://sidecar:8080", "test-relayer");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chainlinkCcv.ccv"));
    }

    #[test]
    fn test_parse_duration_variants() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("2s"), Some(Duration::from_secs(2)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("60"), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration("invalid"), None);
    }

    // Helper to create a valid test config
    fn test_config() -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
                read_timeout: default_read_timeout(),
                write_timeout: default_write_timeout(),
                idle_timeout: default_idle_timeout(),
                security: SecurityConfig::default(),
            },
            database: DatabaseConfig {
                path: default_db_path(),
            },
            logging: LoggingConfig {
                level: default_log_level(),
                format: default_log_format(),
            },
            symbiotic_relay: SymbioticRelayConfig {
                address: "http://localhost:50051".to_string(),
                key_tag: default_key_tag(),
                use_mock: true,
                max_retries: default_max_retries(),
                timeout: default_timeout(),
                retry_backoff: default_retry_backoff(),
            },
            signer: SignerConfig {
                event_poll_interval: default_event_poll_interval(),
                sign_job_interval: default_sign_job_interval(),
                sign_worker_count: default_sign_worker_count(),
                min_batch_size: default_min_batch_size(),
                acceptance_hooks: Vec::new(),
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![42161, 31338],
            provider: "layerzero".to_string(),
            layerzero: Some(LayerZeroConfig::default()),
            chainlink_ccv: None,
        }
    }
}
