//! OpenZeppelin Relayer client module
//!
//! This module provides HTTP client integration with the OpenZeppelin Relayer
//! for transaction management. It handles:
//! - Submitting transactions to the relayer
//! - Querying transaction status
//! - Idempotency key management

mod client;
mod types;

pub use client::RelayerClient;
pub use types::{
    ChainRelayerConfig, EvmTransactionRequest, Speed, TransactionResponse, TransactionStatus,
};
