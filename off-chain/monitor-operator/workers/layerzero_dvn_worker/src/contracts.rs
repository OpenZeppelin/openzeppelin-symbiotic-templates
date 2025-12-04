use ethers::prelude::*;

// DVN contract ABI bindings
abigen!(
    SymbioticLayerZeroDVN,
    r#"[
        event JobAssigned(bytes32 indexed jobId, uint32 indexed dstEid, bytes32 payloadHash, address sender, bytes packetHeader, uint64 confirmations)
        event VerificationSubmitted(bytes32 indexed packetHash, uint48 epoch, uint64 confirmations)
        function submitVerification(bytes calldata packetHeader, bytes32 payloadHash, uint64 confirmations, uint48 epoch, bytes calldata proof) external
        function receiveUln() external view returns (address)
    ]"#
);

/// Parsed JobAssigned event data
#[derive(Debug, Clone)]
pub struct JobAssignedEvent {
    pub job_id: [u8; 32],
    pub dst_eid: u32,
    pub payload_hash: [u8; 32],
    pub sender: Address,
    pub packet_header: Vec<u8>,
    pub confirmations: u64,
}

impl JobAssignedEvent {
    /// Parse from raw log
    pub fn from_log(log: &Log) -> Option<Self> {
        // JobAssigned event signature: keccak256("JobAssigned(bytes32,uint32,bytes32,address,bytes,uint64)")
        let event_sig = H256::from_slice(&ethers::utils::keccak256(
            "JobAssigned(bytes32,uint32,bytes32,address,bytes,uint64)",
        ));

        if log.topics.get(0)? != &event_sig {
            return None;
        }

        // Indexed parameters are in topics
        let job_id = log.topics.get(1)?.as_bytes().try_into().ok()?;

        // dst_eid is indexed but uint32, so it's padded to 32 bytes
        let dst_eid_bytes = log.topics.get(2)?;
        let dst_eid = u32::from_be_bytes(dst_eid_bytes.as_bytes()[28..32].try_into().ok()?);

        // Non-indexed parameters are in data
        // Layout: payloadHash (32), sender (32 padded), packetHeader (dynamic), confirmations (32 padded)
        let data = log.data.as_ref();

        if data.len() < 128 {
            return None;
        }

        // payloadHash at offset 0
        let payload_hash: [u8; 32] = data[0..32].try_into().ok()?;

        // sender at offset 32 (address is 20 bytes, right-padded in 32-byte slot)
        let sender = Address::from_slice(&data[44..64]);

        // packetHeader offset pointer at offset 64
        let header_offset = U256::from_big_endian(&data[64..96]).as_usize();

        // confirmations at offset 96
        let confirmations = U256::from_big_endian(&data[96..128]).as_u64();

        // Read dynamic packetHeader data
        // First 32 bytes at offset are the length
        let header_len = U256::from_big_endian(&data[header_offset..header_offset + 32]).as_usize();
        let packet_header = data[header_offset + 32..header_offset + 32 + header_len].to_vec();

        Some(JobAssignedEvent {
            job_id,
            dst_eid,
            payload_hash,
            sender,
            packet_header,
            confirmations,
        })
    }
}

/// Configuration for a destination chain
#[derive(Debug, Clone)]
pub struct DestinationConfig {
    pub chain_id: u64,
    pub eid: u32,
    pub rpc_url: String,
    pub dvn_address: Address,
}

impl DestinationConfig {
    pub fn new(chain_id: u64, eid: u32, rpc_url: &str, dvn_address: Address) -> Self {
        Self {
            chain_id,
            eid,
            rpc_url: rpc_url.to_string(),
            dvn_address,
        }
    }
}
