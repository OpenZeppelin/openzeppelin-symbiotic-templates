use alloy::primitives::{Address, B256};
use alloy::rpc::types::Log;
use alloy::sol;
use alloy::sol_types::SolEvent;
use eyre::Result;
use serde::{Deserialize, Serialize};

// Define LayerZero DVN JobAssigned event using sol! macro for compile-time ABI
// This is the Symbiotic spec 11-field version from SymbioticLayerZeroDVN.sol
sol! {
    #[derive(Debug)]
    event JobAssigned(
        bytes32 indexed guid,      // Globally unique identifier (use as message ID)
        uint32 srcEid,             // Source endpoint ID
        uint32 dstEid,             // Destination endpoint ID
        address sender,            // Sender address on source chain
        bytes32 receiver,          // Receiver address on destination (as bytes32)
        bytes32 payloadHash,       // Hash of the message payload
        bytes packetHeader,        // LayerZero packet header (81 bytes)
        uint64 confirmations,      // Required block confirmations
        uint64 nonce,              // Message nonce
        bytes options,             // Execution options
        uint256 fee                // Fee paid for verification
    );
}

/// Get the topic0 for JobAssigned event
pub fn job_assigned_topic() -> B256 {
    JobAssigned::SIGNATURE_HASH
}

/// Decoded JobAssigned event - serializable version (DVN 11-field format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedJobAssigned {
    /// Globally unique identifier - use as message ID
    pub guid: B256,
    /// Source endpoint ID (LayerZero EID)
    pub src_eid: u32,
    /// Destination endpoint ID (LayerZero EID)
    pub dst_eid: u32,
    /// Sender address on source chain
    pub sender: Address,
    /// Receiver address on destination chain (as bytes32)
    pub receiver: B256,
    /// Hash of the message payload
    pub payload_hash: B256,
    /// LayerZero packet header (81 bytes)
    #[serde(with = "hex::serde")]
    pub packet_header: Vec<u8>,
    /// Required block confirmations
    pub confirmations: u64,
    /// Message nonce
    pub nonce: u64,
    /// Execution options
    #[serde(with = "hex::serde")]
    pub options: Vec<u8>,
    /// Fee paid for verification
    pub fee: alloy::primitives::U256,
}

impl DecodedJobAssigned {
    /// Decode a JobAssigned event from a log
    pub fn decode_log(log: &Log) -> Result<Self> {
        // Convert alloy rpc Log to primitives Log for decoding
        let primitive_log = alloy::primitives::Log {
            address: log.inner.address,
            data: log.inner.data.clone(),
        };

        let decoded = JobAssigned::decode_log(&primitive_log, true)?;

        Ok(Self {
            guid: decoded.guid,
            src_eid: decoded.srcEid,
            dst_eid: decoded.dstEid,
            sender: decoded.sender,
            receiver: decoded.receiver,
            payload_hash: decoded.payloadHash,
            packet_header: decoded.packetHeader.to_vec(),
            confirmations: decoded.confirmations,
            nonce: decoded.nonce,
            options: decoded.options.to_vec(),
            fee: decoded.fee,
        })
    }

    /// Get the message ID (guid is the unique identifier)
    pub fn message_id(&self) -> B256 {
        self.guid
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_assigned_topic() {
        let topic = job_assigned_topic();
        // Verify it's a valid 32-byte hash
        assert_eq!(topic.len(), 32);
    }
}
