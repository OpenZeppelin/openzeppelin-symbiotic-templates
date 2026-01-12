use std::collections::HashMap;
use std::time::Duration;

use config::{Config, Environment, File};
use serde::Deserialize;

use crate::error::ConfigError;

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
/// Note: Secrets (api_key, webhook_secret) are read from env vars, not config.
/// Set OZ_RELAYER_API_KEY and OZ_RELAYER_WEBHOOK_SECRET environment variables.
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
    /// DVN contract address on this chain
    pub dvn_address: String,
}

impl Default for OzRelayerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8080".to_string(),
            poll_interval: default_oz_poll_interval(),
            status_poll_interval: default_oz_status_poll_interval(),
            default_speed: default_oz_speed(),
            timeout: default_oz_timeout(),
            chain_relayers: Vec::new(),
        }
    }
}

/// Security configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// API key for general authentication (empty = disabled)
    pub api_key: Option<String>,
    /// HMAC secret for webhook signature verification (empty = disabled)
    pub webhook_secret: Option<String>,
    /// Maximum age for request timestamps
    #[serde(with = "humantime_serde", default = "default_timestamp_window")]
    pub timestamp_window: Duration,
    #[serde(default)]
    pub enable_cors: bool,
    /// Enable debug endpoints (/debug/v1/*). Disable in production.
    #[serde(default = "default_enable_debug_endpoints")]
    pub enable_debug_endpoints: bool,
}

fn default_enable_debug_endpoints() -> bool {
    true // Backwards compatible default, should be false in production
}

/// LayerZero provider configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LayerZeroConfig {
    /// Maps LayerZero Endpoint IDs (EID) to chain IDs
    #[serde(default)]
    pub eid_to_chain_id: HashMap<u32, u64>,
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
