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

// Define CCIP OnRamp CCIPMessageSent event.
sol! {
    #[derive(Debug)]
    struct CcipReceipt {
        address issuer;
        uint32 destGasLimit;
        uint32 destBytesOverhead;
        uint256 feeTokenAmount;
        bytes extraArgs;
    }

    #[derive(Debug)]
    event CCIPMessageSent(
        uint64 indexed destChainSelector,
        address indexed sender,
        bytes32 indexed messageId,
        address feeToken,
        uint256 tokenAmountBeforeTokenPoolFees,
        bytes encodedMessage,
        CcipReceipt[] receipts,
        bytes[] verifierBlobs
    );

    /// Emitted by the destination OffRamp once it has dispatched a message to
    /// the receiver. Conveys the *message-level* outcome — whether the
    /// receiver's callback succeeded — which the outer tx status alone does
    /// not (OffRamp catches receiver reverts and lets the tx succeed).
    ///
    /// The `state` field follows `Internal.MessageExecutionState`:
    /// 0 = Untouched, 1 = InProgress, 2 = Success, 3 = Failure.
    ///
    /// NOTE: signature must match the deployed CCIP OffRamp exactly. The
    /// canonical CCIP v2 staging event is the 5-field form (no gasUsed). On
    /// Sepolia this hashes to topic0 = 0x8c324ce1367b83031769f6a813e3bb4c117aba2185789d66b98b791405be6df2.
    #[derive(Debug)]
    event ExecutionStateChanged(
        uint64 indexed sourceChainSelector,
        uint64 indexed sequenceNumber,
        bytes32 indexed messageId,
        uint8 state,
        bytes returnData
    );
}

/// Get the topic0 for JobAssigned event
pub fn job_assigned_topic() -> B256 {
    JobAssigned::SIGNATURE_HASH
}

/// Get the topic0 for CCIPMessageSent event.
pub fn ccip_message_sent_topic() -> B256 {
    CCIPMessageSent::SIGNATURE_HASH
}

/// Get the topic0 for the destination OffRamp's ExecutionStateChanged event.
pub fn ccip_execution_state_changed_topic() -> B256 {
    ExecutionStateChanged::SIGNATURE_HASH
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

/// Decoded CCIPMessageSent event - serializable subset used by the operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedCcipMessageSent {
    pub dest_chain_selector: u64,
    pub sender: Address,
    pub message_id: B256,
    pub fee_token: Address,
    #[serde(with = "hex::serde")]
    pub encoded_message: Vec<u8>,
    pub verifier_blobs: Vec<Vec<u8>>,
}

impl DecodedCcipMessageSent {
    /// Decode a CCIPMessageSent event from a log.
    pub fn decode_log(log: &Log) -> Result<Self> {
        let primitive_log = alloy::primitives::Log {
            address: log.inner.address,
            data: log.inner.data.clone(),
        };

        let decoded = CCIPMessageSent::decode_log(&primitive_log, true)?;

        Ok(Self {
            dest_chain_selector: decoded.destChainSelector,
            sender: decoded.sender,
            message_id: decoded.messageId,
            fee_token: decoded.feeToken,
            encoded_message: decoded.encodedMessage.to_vec(),
            verifier_blobs: decoded
                .verifierBlobs
                .iter()
                .map(|blob| blob.to_vec())
                .collect(),
        })
    }
}

/// Decoded ExecutionStateChanged event - serializable subset used by the operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedExecutionStateChanged {
    pub source_chain_selector: u64,
    pub sequence_number: u64,
    pub message_id: B256,
    pub state: u8,
}

impl DecodedExecutionStateChanged {
    pub fn decode_log(log: &Log) -> Result<Self> {
        let primitive_log = alloy::primitives::Log {
            address: log.inner.address,
            data: log.inner.data.clone(),
        };
        let decoded = ExecutionStateChanged::decode_log(&primitive_log, true)?;
        Ok(Self {
            source_chain_selector: decoded.sourceChainSelector,
            sequence_number: decoded.sequenceNumber,
            message_id: decoded.messageId,
            state: decoded.state,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_job_assigned_topic() {
        let topic = job_assigned_topic();
        // Verify it's a valid 32-byte hash
        assert_eq!(topic.len(), 32);
    }

    #[test]
    fn test_job_assigned_topic_is_deterministic() {
        let topic1 = job_assigned_topic();
        let topic2 = job_assigned_topic();
        assert_eq!(topic1, topic2);
    }

    #[test]
    fn test_decoded_job_assigned_message_id() {
        let guid = B256::from_slice(&[0xAAu8; 32]);
        let job = DecodedJobAssigned {
            guid,
            src_eid: 40231,
            dst_eid: 40232,
            sender: Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x03u8; 32]),
            packet_header: vec![0u8; 81],
            confirmations: 15,
            nonce: 1,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        // message_id should return guid
        assert_eq!(job.message_id(), guid);
    }

    #[test]
    fn test_decoded_job_assigned_serialization() {
        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x01u8; 32]),
            src_eid: 30101,
            dst_eid: 30110,
            sender: Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::from_slice(&[0x02u8; 32]),
            packet_header: vec![0x11, 0x22, 0x33],
            confirmations: 10,
            nonce: 42,
            options: vec![0xAA, 0xBB],
            fee: alloy::primitives::U256::from(1000u64),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&job).unwrap();
        let deserialized: DecodedJobAssigned = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.guid, job.guid);
        assert_eq!(deserialized.src_eid, job.src_eid);
        assert_eq!(deserialized.dst_eid, job.dst_eid);
        assert_eq!(deserialized.confirmations, job.confirmations);
        assert_eq!(deserialized.nonce, job.nonce);
        assert_eq!(deserialized.packet_header, job.packet_header);
        assert_eq!(deserialized.options, job.options);
    }

    #[test]
    fn test_decoded_job_assigned_all_fields() {
        let guid = B256::from_slice(&[0x11u8; 32]);
        let sender = Address::from_slice(&[0x22u8; 20]);
        let receiver = B256::from_slice(&[0x33u8; 32]);
        let payload_hash = B256::from_slice(&[0x44u8; 32]);
        let packet_header = vec![0u8; 81];
        let options = vec![0x01, 0x02, 0x03];
        let fee = alloy::primitives::U256::from(5000u64);

        let job = DecodedJobAssigned {
            guid,
            src_eid: 30101,
            dst_eid: 30110,
            sender,
            receiver,
            payload_hash,
            packet_header: packet_header.clone(),
            confirmations: 20,
            nonce: 100,
            options: options.clone(),
            fee,
        };

        assert_eq!(job.guid, guid);
        assert_eq!(job.src_eid, 30101);
        assert_eq!(job.dst_eid, 30110);
        assert_eq!(job.sender, sender);
        assert_eq!(job.receiver, receiver);
        assert_eq!(job.payload_hash, payload_hash);
        assert_eq!(job.packet_header, packet_header);
        assert_eq!(job.confirmations, 20);
        assert_eq!(job.nonce, 100);
        assert_eq!(job.options, options);
        assert_eq!(job.fee, fee);
    }

    #[test]
    fn test_decoded_job_assigned_clone() {
        let job = DecodedJobAssigned {
            guid: B256::from_slice(&[0x01u8; 32]),
            src_eid: 40231,
            dst_eid: 40232,
            sender: Address::ZERO,
            receiver: B256::ZERO,
            payload_hash: B256::ZERO,
            packet_header: vec![],
            confirmations: 15,
            nonce: 1,
            options: vec![],
            fee: alloy::primitives::U256::ZERO,
        };

        let cloned = job.clone();
        assert_eq!(cloned.guid, job.guid);
        assert_eq!(cloned.src_eid, job.src_eid);
    }

    #[test]
    fn test_ccip_message_sent_topic_is_deterministic() {
        let topic1 = ccip_message_sent_topic();
        let topic2 = ccip_message_sent_topic();
        assert_eq!(topic1, topic2);
        assert_eq!(topic1.len(), 32);
    }

    /// Pin the destination OffRamp event hash to the value observed on Sepolia
    /// CCIP v2 staging. If this assertion ever changes, our event struct has
    /// drifted from the deployed contract — every signature character matters
    /// (e.g. a phantom uint256 field would silently break monitor matching).
    #[test]
    fn test_ccip_execution_state_changed_topic_pinned() {
        let topic = ccip_execution_state_changed_topic();
        let expected: B256 = "0x8c324ce1367b83031769f6a813e3bb4c117aba2185789d66b98b791405be6df2"
            .parse()
            .unwrap();
        assert_eq!(topic, expected);
    }

    #[test]
    fn test_decoded_ccip_message_sent_fields() {
        let evt = DecodedCcipMessageSent {
            dest_chain_selector: 31338,
            sender: Address::ZERO,
            message_id: B256::from_slice(&[0x11u8; 32]),
            fee_token: Address::ZERO,
            encoded_message: vec![0x01, 0x02],
            verifier_blobs: vec![vec![0xaa, 0xbb]],
        };

        assert_eq!(evt.dest_chain_selector, 31338);
        assert_eq!(evt.message_id, B256::from_slice(&[0x11u8; 32]));
        assert_eq!(evt.encoded_message, vec![0x01, 0x02]);
        assert_eq!(evt.verifier_blobs.len(), 1);
    }

    // ============ decode_log round-trip tests ============

    /// Helper: build an alloy Log from the given topics and ABI-encoded data.
    fn build_log(address: Address, topics: Vec<B256>, data: Vec<u8>) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address,
                data: alloy::primitives::LogData::new_unchecked(
                    topics,
                    alloy::primitives::Bytes::from(data),
                ),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: None,
            transaction_index: None,
            log_index: None,
            removed: false,
        }
    }

    #[test]
    fn test_decode_job_assigned_log_round_trip() {
        use alloy::sol_types::SolEvent;

        let guid = B256::from_slice(&[0xAAu8; 32]);
        let src_eid: u32 = 30101;
        let dst_eid: u32 = 30110;
        let sender = Address::from_slice(&[0x11u8; 20]);
        let receiver = B256::from_slice(&[0x22u8; 32]);
        let payload_hash = B256::from_slice(&[0x33u8; 32]);
        let packet_header = vec![0u8; 81];
        let confirmations: u64 = 15;
        let nonce: u64 = 42;
        let options = vec![0x01, 0x02];
        let fee = alloy::primitives::U256::from(5000u64);

        let event = JobAssigned {
            guid,
            srcEid: src_eid,
            dstEid: dst_eid,
            sender,
            receiver,
            payloadHash: payload_hash,
            packetHeader: alloy::primitives::Bytes::from(packet_header.clone()),
            confirmations,
            nonce,
            options: alloy::primitives::Bytes::from(options.clone()),
            fee,
        };

        let encoded = event.encode_log_data();
        let log = build_log(
            Address::ZERO,
            encoded.topics().to_vec(),
            encoded.data.to_vec(),
        );

        let decoded = DecodedJobAssigned::decode_log(&log).unwrap();
        assert_eq!(decoded.guid, guid);
        assert_eq!(decoded.src_eid, src_eid);
        assert_eq!(decoded.dst_eid, dst_eid);
        assert_eq!(decoded.sender, sender);
        assert_eq!(decoded.receiver, receiver);
        assert_eq!(decoded.payload_hash, payload_hash);
        assert_eq!(decoded.packet_header, packet_header);
        assert_eq!(decoded.confirmations, confirmations);
        assert_eq!(decoded.nonce, nonce);
        assert_eq!(decoded.options, options);
        assert_eq!(decoded.fee, fee);
        assert_eq!(decoded.message_id(), guid);
    }

    #[test]
    fn test_decode_ccip_message_sent_log_round_trip() {
        use alloy::sol_types::SolEvent;

        let dest_chain_selector: u64 = 31338;
        let sender = Address::from_slice(&[0x11u8; 20]);
        let message_id = B256::from_slice(&[0x22u8; 32]);
        let fee_token = Address::from_slice(&[0x33u8; 20]);
        let encoded_message = vec![0x01, 0x02, 0x03];
        let verifier_blobs: Vec<alloy::primitives::Bytes> =
            vec![alloy::primitives::Bytes::from(vec![0xaa, 0xbb])];

        let event = CCIPMessageSent {
            destChainSelector: dest_chain_selector,
            sender,
            messageId: message_id,
            feeToken: fee_token,
            tokenAmountBeforeTokenPoolFees: alloy::primitives::U256::ZERO,
            encodedMessage: alloy::primitives::Bytes::from(encoded_message.clone()),
            receipts: vec![],
            verifierBlobs: verifier_blobs.clone(),
        };

        let encoded = event.encode_log_data();
        let log = build_log(
            Address::ZERO,
            encoded.topics().to_vec(),
            encoded.data.to_vec(),
        );

        let decoded = DecodedCcipMessageSent::decode_log(&log).unwrap();
        assert_eq!(decoded.dest_chain_selector, dest_chain_selector);
        assert_eq!(decoded.sender, sender);
        assert_eq!(decoded.message_id, message_id);
        assert_eq!(decoded.fee_token, fee_token);
        assert_eq!(decoded.encoded_message, encoded_message);
        assert_eq!(decoded.verifier_blobs.len(), 1);
        assert_eq!(decoded.verifier_blobs[0], vec![0xaa, 0xbb]);
    }

    #[test]
    fn test_decode_job_assigned_wrong_topic() {
        // A log whose topic0 does not match JobAssigned
        let wrong_topic = B256::from_slice(&[0xFFu8; 32]);
        let log = build_log(Address::ZERO, vec![wrong_topic], vec![0u8; 320]);

        let result = DecodedJobAssigned::decode_log(&log);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_job_assigned_too_few_topics() {
        // An empty-topics log should fail
        let log = build_log(Address::ZERO, vec![], vec![]);

        let result = DecodedJobAssigned::decode_log(&log);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_job_assigned_malformed_data() {
        // Correct topic0 but garbage data
        let topic0 = job_assigned_topic();
        let log = build_log(Address::ZERO, vec![topic0, B256::ZERO], vec![0xDE, 0xAD]);

        let result = DecodedJobAssigned::decode_log(&log);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_ccip_message_sent_wrong_topic() {
        let wrong_topic = B256::from_slice(&[0xFFu8; 32]);
        let log = build_log(Address::ZERO, vec![wrong_topic], vec![0u8; 320]);

        let result = DecodedCcipMessageSent::decode_log(&log);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_ccip_message_sent_malformed_data() {
        let topic0 = ccip_message_sent_topic();
        let log = build_log(
            Address::ZERO,
            vec![topic0, B256::ZERO, B256::ZERO, B256::ZERO],
            vec![0xDE, 0xAD],
        );

        let result = DecodedCcipMessageSent::decode_log(&log);
        assert!(result.is_err());
    }
}
