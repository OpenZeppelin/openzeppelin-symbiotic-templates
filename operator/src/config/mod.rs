use std::time::Duration;

use config::{Config, Environment, File};
use serde::Deserialize;

use crate::error::ConfigError;
pub use crate::provider::types::{ChainlinkCcvConfig, LayerZeroConfig};

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
#[derive(Debug, Clone, Deserialize, Default)]
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
    true // Backwards compatible default, should be false in production
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

impl AppConfig {
    /// Load configuration from file and environment variables
    pub fn load(config_path: Option<&str>) -> Result<Self, ConfigError> {
        let mut builder = Config::builder();

        // Load from config file if provided
        if let Some(path) = config_path {
            builder = builder.add_source(File::with_name(path).required(false));
        }

        // Override with environment variables
        // Format: <SECTION>_<KEY> (e.g., SERVER_PORT, OZ_RELAYER_API_KEY)
        builder = builder.add_source(
            Environment::default()
                .separator("_")
                .try_parsing(true),
        );

        let config = builder.build()?;
        let app_config: AppConfig = config.try_deserialize()?;

        // Validate configuration
        app_config.validate()?;

        Ok(app_config)
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
        assert!(matches!(err, ConfigError::Validation(msg) if msg.contains("OZ_RELAYER_WEBHOOK_SECRET")));
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
        assert!(matches!(result.unwrap_err(), ConfigError::Validation(msg) if msg.contains("port")));
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
        assert!(default_enable_debug_endpoints());
    }

    #[test]
    fn test_layerzero_config_default() {
        let config = LayerZeroConfig::default();
        assert!(config.eid_to_chain_id.is_empty());
        assert!(config.dvn_addresses.is_empty());
    }

    #[test]
    fn test_app_config_valid() {
        let config = test_config();
        assert!(config.validate().is_ok());
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
            },
            oz_relayer: OzRelayerConfig::default(),
            destination_chains: vec![42161, 31338],
            provider: "layerzero".to_string(),
            layerzero: Some(LayerZeroConfig::default()),
            chainlink_ccv: None,
        }
    }
}
