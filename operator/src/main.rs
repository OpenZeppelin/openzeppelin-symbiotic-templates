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
use tracing_subscriber::{fmt, EnvFilter};

mod api;
mod config;
mod crypto;
mod error;
mod evm;
mod provider;
mod symbiotic_relay;
mod relay_submitter;
mod relayer_client;
mod signer;
mod storage;
mod submitter;
mod webhook;

use api::{create_router, AppState, SecurityState};
use config::AppConfig;
use provider::{create_provider, DynProvider};
use symbiotic_relay::{MockSymbioticRelayClient, SymbioticRelayClient, SymbioticRelayClientEnum};
use relay_submitter::RelaySubmitterJob;
use relayer_client::{ChainRelayerConfig, RelayerClient as OzRelayerClient};
use signer::SignerJob;
use storage::Storage;

/// Symbiotic DVN Operator
#[derive(Parser, Debug)]
#[command(name = "operator")]
#[command(about = "Symbiotic DVN operator for cross-chain message attestation")]
#[command(version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.yaml")]
    config: String,
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

    // Load configuration and wrap in Arc to avoid deep cloning
    let config = Arc::new(
        AppConfig::load(Some(&args.config))
            .wrap_err_with(|| format!("failed to load config from {}", args.config))?,
    );

    // Initialize logging
    init_logging(&config.logging.level, &config.logging.format);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config_path = %args.config,
        "starting operator"
    );

    // Initialize storage
    let storage = Arc::new(
        Storage::new(&config.database.path)
            .wrap_err_with(|| format!("failed to open database at {}", config.database.path))?,
    );
    tracing::info!(path = %config.database.path, "initialized storage");

    // Initialize provider
    let provider: DynProvider =
        create_provider(Arc::clone(&config), Arc::clone(&storage))
            .wrap_err_with(|| format!("failed to initialize {} provider", config.provider))?;
    tracing::info!(provider = provider.name(), "initialized provider");

    // Initialize OZ Relayer client (for proof submission via HTTP API)
    let oz_relayer_client = if !config.oz_relayer.chain_relayers.is_empty() {
        // Read API key from environment (secrets not stored in config files)
        let api_key = env::var("OZ_RELAYER_API_KEY")
            .expect("OZ_RELAYER_API_KEY must be set when oz_relayer.chain_relayers is configured");

        let chain_configs: Vec<ChainRelayerConfig> = config
            .oz_relayer
            .chain_relayers
            .iter()
            .map(|entry| {
                ChainRelayerConfig::new(
                    entry.chain_id,
                    entry.relayer_id.clone(),
                    entry.dvn_address.clone(),
                )
            })
            .collect();

        let client = OzRelayerClient::new(
            config.oz_relayer.base_url.clone(),
            api_key,
            chain_configs.clone(),
            config.oz_relayer.timeout,
        )
        .wrap_err("failed to create OZ Relayer client")?;

        for chain_config in &chain_configs {
            tracing::info!(
                chain_id = chain_config.chain_id,
                relayer_id = %chain_config.relayer_id,
                dvn = %chain_config.dvn_address,
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
            .wrap_err_with(|| format!("failed to connect to symbiotic relay at {}", config.symbiotic_relay.address))?;
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
    let signer_job = SignerJob::new(
        Arc::clone(&storage),
        provider.clone(),
        Arc::clone(&config),
    );

    let signer_shutdown_rx = shutdown_tx.subscribe();
    let signer_handle = tokio::spawn(async move {
        if let Err(e) = signer_job.run(symbiotic_relay_client, signer_shutdown_rx).await {
            tracing::error!(error = %e, "signer job error");
        }
    });

    // Start relay submitter job (if OZ Relayer is configured)
    let submitter_handle = if let Some(oz_client) = oz_relayer_client {
        let relay_submitter = RelaySubmitterJob::new(
            Arc::clone(&storage),
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
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("server error");
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
