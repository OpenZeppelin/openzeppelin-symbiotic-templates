#![deny(clippy::unwrap_used)]

use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
use eyre::WrapErr;
use tokio::signal;
use tokio::sync::broadcast;
use tower_http::timeout::TimeoutLayer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

mod api;
mod config;
mod crypto;
mod error;
mod evm;
mod provider;
mod relay_submitter;
mod relayer_client;
mod signer;
mod storage;
mod submitter;
mod symbiotic_relay;
mod webhook;

use api::{AppState, SecurityState, create_router};
use config::AppConfig;
use provider::{DynProvider, create_provider};
use relay_submitter::RelaySubmitterJob;
use relayer_client::{ChainRelayerConfig, RelayerClient as OzRelayerClient};
use signer::SignerJob;
use storage::Storage;
use symbiotic_relay::{MockSymbioticRelayClient, SymbioticRelayClient, SymbioticRelayClientEnum};

/// Symbiotic DVN Operator
#[derive(Parser, Debug)]
#[command(name = "operator")]
#[command(about = "Symbiotic DVN operator for cross-chain message attestation")]
#[command(version)]
struct Args {
    /// Path to environment JSON config
    #[arg(short, long)]
    environment: String,

    /// Path to deployments JSON config
    #[arg(long)]
    deployments: String,

    /// Operator index (1-based), used with the environment/deployments pair to derive sidecar URL and relayer ID
    #[arg(long, default_value = "1")]
    operator_index: u8,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Parse command line arguments with explicit flush for Docker compatibility
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            let _ = e.print();
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(e.exit_code());
        }
    };

    let mut config =
        AppConfig::load_from_paths(&args.environment, &args.deployments, args.operator_index)
            .wrap_err_with(|| {
                format!(
                    "failed to load config from environment {} and deployments {}",
                    args.environment, args.deployments
                )
            })?;

    // Load security secrets from environment variables (required)
    config
        .load_security_secrets_from_env()
        .wrap_err("failed to load security secrets from environment")?;

    // Validate OZ_RELAYER_API_KEY is set (required for relayer authentication)
    let oz_relayer_api_key = env::var("OZ_RELAYER_API_KEY")
        .wrap_err("OZ_RELAYER_API_KEY environment variable is required")?;

    // Validate security configuration (fails startup if invalid)
    config
        .server
        .security
        .validate()
        .wrap_err("security configuration validation failed")?;

    let config = Arc::new(config);

    // Initialize logging
    init_logging(&config.logging.level, &config.logging.format);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config_source = %args.environment,
        deployments_source = %args.deployments,
        operator_index = args.operator_index,
        "starting operator"
    );

    tracing::info!(
        webhook_secret_len = config
            .server
            .security
            .webhook_secret
            .as_ref()
            .map(|s| s.len())
            .unwrap_or(0),
        oz_relayer_webhook_secret_len = config
            .server
            .security
            .oz_relayer_webhook_secret
            .as_ref()
            .map(|s| s.len())
            .unwrap_or(0),
        debug_endpoints = config.server.security.enable_debug_endpoints,
        "security configuration loaded"
    );

    // Initialize storage
    let storage = Arc::new(
        Storage::new_with_provider(&config.database.path, &config.provider)
            .wrap_err_with(|| format!("failed to open database at {}", config.database.path))?,
    );
    tracing::info!(path = %config.database.path, provider = %config.provider, "initialized storage");

    // Initialize provider
    let provider: DynProvider = create_provider(Arc::clone(&config), Arc::clone(&storage))
        .wrap_err_with(|| format!("failed to initialize {} provider", config.provider))?;
    tracing::info!(provider = provider.name(), "initialized provider");

    // Initialize OZ Relayer client (for proof submission via HTTP API)
    let oz_relayer_client = if !config.oz_relayer.chain_relayers.is_empty() {
        let chain_configs: Vec<ChainRelayerConfig> = config
            .oz_relayer
            .chain_relayers
            .iter()
            .map(|entry| {
                ChainRelayerConfig::new(
                    entry.chain_id,
                    entry.relayer_id.clone(),
                    entry.target_address.clone(),
                )
            })
            .collect();

        let client = OzRelayerClient::new(
            config.oz_relayer.base_url.clone(),
            oz_relayer_api_key.clone(),
            chain_configs.clone(),
            config.oz_relayer.timeout,
            config.oz_relayer.max_retries,
            config.oz_relayer.retry_backoff,
        )
        .wrap_err("failed to create OZ Relayer client")?;

        for chain_config in &chain_configs {
            tracing::info!(
                chain_id = chain_config.chain_id,
                relayer_id = %chain_config.relayer_id,
                target = %chain_config.target_address,
                "configured OZ Relayer for chain"
            );
        }

        Some(client)
    } else {
        tracing::info!("OZ Relayer not configured (no chain_relayers)");
        None
    };

    // Create Symbiotic relay client (for BLS signature aggregation)
    let symbiotic_relay_client = if config.symbiotic_relay.use_mock {
        tracing::info!("using mock symbiotic relay client");
        SymbioticRelayClientEnum::Mock(MockSymbioticRelayClient::new())
    } else {
        tracing::info!(address = %config.symbiotic_relay.address, "connecting to symbiotic relay");
        let client = SymbioticRelayClient::new(config.symbiotic_relay.clone())
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to connect to symbiotic relay at {}",
                    config.symbiotic_relay.address
                )
            })?;
        SymbioticRelayClientEnum::Real(client)
    };

    // Create shutdown signal channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Create application state
    let app_state = AppState {
        storage: Arc::clone(&storage),
        provider: provider.clone(),
        config: Arc::clone(&config),
        start_time: Instant::now(),
    };

    // Create router
    let router = create_router(app_state);

    // Add security middleware layers
    let security_state = SecurityState {
        config: config.server.security.clone(),
    };

    // Add CORS middleware (checks enable_cors internally)
    let router = router.layer(axum::middleware::from_fn_with_state(
        security_state.clone(),
        api::cors_middleware,
    ));

    // Add security middleware (handles API key, webhook auth, and debug endpoint blocking)
    let router = router.layer(axum::middleware::from_fn_with_state(
        security_state,
        api::security_middleware,
    ));

    // Add request timeout layer (uses write_timeout from config)
    // This applies a timeout to the entire request handling, returning 408 if exceeded
    let router = router.layer(TimeoutLayer::new(config.server.write_timeout));

    tracing::info!(
        read_timeout = ?config.server.read_timeout,
        write_timeout = ?config.server.write_timeout,
        idle_timeout = ?config.server.idle_timeout,
        "configured HTTP server timeouts"
    );

    // Bind server address
    let server_addr = config.server_address();
    let addr: SocketAddr = server_addr
        .parse()
        .wrap_err_with(|| format!("invalid server address: {}", server_addr))?;
    tracing::info!(%addr, "starting HTTP server");

    // Create listener
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .wrap_err_with(|| format!("failed to bind to {}", addr))?;

    // Start signer job
    let signer_job = SignerJob::new(Arc::clone(&storage), provider.clone(), Arc::clone(&config));

    let signer_shutdown_rx = shutdown_tx.subscribe();
    let signer_handle = tokio::spawn(async move {
        if let Err(e) = signer_job
            .run(symbiotic_relay_client, signer_shutdown_rx)
            .await
        {
            tracing::error!(error = %e, "signer job error");
        }
    });

    // Start relay submitter job (if OZ Relayer is configured)
    let submitter_handle = if let Some(oz_client) = oz_relayer_client {
        let relay_submitter = RelaySubmitterJob::new(
            Arc::clone(&storage),
            provider.clone(),
            Arc::clone(&config),
            oz_client,
        );

        let submitter_shutdown_rx = shutdown_tx.subscribe();
        Some(tokio::spawn(async move {
            if let Err(e) = relay_submitter.run(submitter_shutdown_rx).await {
                tracing::error!(error = %e, "relay submitter job error");
            }
        }))
    } else {
        tracing::info!("relay submitter job disabled (OZ Relayer not configured)");
        None
    };

    // Start HTTP server with graceful shutdown
    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
        {
            tracing::error!(error = %e, "HTTP server error");
        }
    });

    // Wait for shutdown signal
    shutdown_signal().await;
    tracing::info!("shutdown signal received");

    // Signal all tasks to shutdown
    let _ = shutdown_tx.send(());

    // Wait for tasks to complete with timeout
    let shutdown_timeout = std::time::Duration::from_secs(30);
    tokio::select! {
        _ = tokio::time::sleep(shutdown_timeout) => {
            tracing::warn!("shutdown timeout reached, forcing exit");
        }
        _ = async {
            let _ = signer_handle.await;
            if let Some(handle) = submitter_handle {
                let _ = handle.await;
            }
            let _ = server_handle.await;
        } => {
            tracing::info!("all tasks completed");
        }
    }

    tracing::info!("operator stopped");
    Ok(())
}

/// Initialize logging
fn init_logging(level: &str, format: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    match format {
        "json" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json())
                .init();
        }
        _ => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer())
                .init();
        }
    }
}

/// Shutdown signal handler
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::unwrap_err_used)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn test_args_accept_environment_and_deployments() {
        let args = Args::try_parse_from([
            "operator",
            "--environment",
            "/config/environment.json",
            "--deployments",
            "/config/deployments.json",
            "--operator-index",
            "2",
        ])
        .unwrap();

        assert_eq!(args.environment, "/config/environment.json");
        assert_eq!(args.deployments, "/config/deployments.json");
        assert_eq!(args.operator_index, 2);
    }

    #[test]
    fn test_args_require_environment() {
        let err = Args::try_parse_from(["operator"]).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn test_args_require_deployments_with_environment() {
        let err = Args::try_parse_from(["operator", "--environment", "/config/environment.json"])
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn test_args_require_environment_with_deployments() {
        let err = Args::try_parse_from(["operator", "--deployments", "/config/deployments.json"])
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }
}
