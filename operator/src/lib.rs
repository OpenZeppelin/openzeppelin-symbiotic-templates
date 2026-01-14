//! Symbiotic DVN Operator
//!
//! This crate provides a cross-chain message attestation operator for the Symbiotic LayerZero DVN.
//! It coordinates signing and proof submission for cross-chain message verification.

#![deny(clippy::unwrap_used)]

pub mod api;
pub mod config;
pub mod crypto;
pub mod error;
pub mod evm;
pub mod provider;
pub mod symbiotic_relay;
pub mod relay_submitter;
pub mod relayer_client;
pub mod signer;
pub mod storage;
pub mod submitter;
pub mod webhook;

pub use config::AppConfig;
pub use provider::Provider;
pub use relayer_client::RelayerClient;
pub use storage::Storage;
