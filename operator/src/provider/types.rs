use std::collections::HashMap;

use serde::Deserialize;

/// LayerZero provider configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LayerZeroConfig {
    /// EVM chain id where the source DVN emits JobAssigned.
    #[serde(default)]
    pub source_chain_id: u64,
    /// Source DVN emitter address. Optional for legacy deployments.
    #[serde(default)]
    pub source_dvn_address: Option<String>,
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
}
