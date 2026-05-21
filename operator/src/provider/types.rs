use std::collections::HashMap;

use serde::Deserialize;

/// LayerZero provider configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LayerZeroConfig {
    /// Maps LayerZero Endpoint IDs (EID) to chain IDs
    #[serde(default)]
    pub eid_to_chain_id: HashMap<u32, u64>,
    /// Maps destination chain ID to target contract address on that chain.
    /// Required for computing domain-separated signing hash that matches
    /// on-chain verification: keccak256(abi.encode(chainId, targetAddress, merkleRoot))
    #[serde(default)]
    pub target_addresses: HashMap<u64, String>,
}

/// Chainlink CCV provider configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChainlinkCcvConfig {
    /// EVM chain id where OnRamp emits CCIPMessageSent.
    pub source_chain_id: u64,
    /// EVM chain id where OffRamp.execute is called.
    pub destination_chain_id: u64,
    /// Chainlink chain selector on source.
    pub source_chain_selector: u64,
    /// Chainlink chain selector on destination.
    pub destination_chain_selector: u64,
    /// Source SymbioticCCV contract address.
    pub source_ccv_address: String,
    /// Destination SymbioticCCV contract address.
    pub destination_ccv_address: String,
    /// Source OnRamp address.
    pub source_onramp_address: String,
    /// Destination OffRamp address.
    pub destination_offramp_address: String,
    /// Source-chain executor address that Chainlink's executor expects to see
    /// in `VerifierResult.MessageExecutorAddress` when polling /verifications.
    /// Must match the value Chainlink configures in their executor's
    /// `defaultExecutorAddress[sourceSelector]` map, otherwise the executor
    /// silently drops our results. Optional: when unset the /verifications
    /// endpoint serves empty results (Path A still works unchanged).
    #[serde(default)]
    pub message_executor_address: String,
    /// Verifier name reported in `VerifierResultMetadata.verifierName` and
    /// keyed on by Chainlink's indexer config block. Must match exactly the
    /// `Name` field in their `[[Verifier]]` TOML entry.
    #[serde(default)]
    pub verifier_name: String,
}
