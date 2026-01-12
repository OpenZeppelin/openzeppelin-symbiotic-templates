//! DVN contract bindings for proof submission

use alloy::primitives::{Bytes, B256};
use alloy::sol;
use alloy::sol_types::SolCall;

// DVN contract interface
sol! {
    #[derive(Debug)]
    interface IDVN {
        function submitProof(
            bytes calldata packetHeader,
            bytes32 payloadHash,
            uint64 confirmations,
            bytes32[] calldata merkleProof,
            bytes32 merkleRoot,
            bytes calldata signature
        ) external;

        function computeLeaf(
            bytes calldata packetHeader,
            bytes32 payloadHash,
            uint64 confirmations
        ) external pure returns (bytes32);

        function isLeafVerified(bytes32 leaf) external view returns (bool);
        function isRootVerified(bytes32 root) external view returns (bool);

        error AlreadyVerified();
        error InvalidMerkleProof();
        error SignatureRequired();
        error OnlySubmitter();
    }
}

/// Build signature bytes for DVN submitProof
/// Format: epoch (6 bytes big-endian) + BLS signature
pub fn build_signature(epoch: u64, bls_proof: &[u8]) -> Bytes {
    let mut sig = Vec::with_capacity(6 + bls_proof.len());
    // Epoch is u48 in contract, take lower 6 bytes of u64 big-endian
    sig.extend_from_slice(&epoch.to_be_bytes()[2..8]);
    sig.extend_from_slice(bls_proof);
    Bytes::from(sig)
}

/// Encode submitProof call data
pub fn encode_submit_proof(
    packet_header: &[u8],
    payload_hash: B256,
    confirmations: u64,
    merkle_proof: Vec<B256>,
    merkle_root: B256,
    signature: Bytes,
) -> Bytes {
    let call = IDVN::submitProofCall {
        packetHeader: Bytes::copy_from_slice(packet_header),
        payloadHash: payload_hash,
        confirmations,
        merkleProof: merkle_proof,
        merkleRoot: merkle_root,
        signature,
    };
    Bytes::from(call.abi_encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_signature() {
        let epoch: u64 = 12345;
        let bls_proof = vec![0x01, 0x02, 0x03, 0x04];

        let sig = build_signature(epoch, &bls_proof);

        // First 6 bytes should be epoch in big-endian
        assert_eq!(sig.len(), 6 + bls_proof.len());
        // Epoch 12345 = 0x3039, so bytes should be [0, 0, 0, 0, 0x30, 0x39]
        assert_eq!(&sig[0..6], &[0, 0, 0, 0, 0x30, 0x39]);
        assert_eq!(&sig[6..], &bls_proof);
    }

    #[test]
    fn test_encode_submit_proof() {
        let packet_header = vec![0x01u8; 81];
        let payload_hash = B256::from_slice(&[0x02u8; 32]);
        let confirmations = 15u64;
        let merkle_proof = vec![B256::from_slice(&[0x03u8; 32])];
        let merkle_root = B256::from_slice(&[0x04u8; 32]);
        let signature = Bytes::from(vec![0x05u8; 100]);

        let calldata = encode_submit_proof(
            &packet_header,
            payload_hash,
            confirmations,
            merkle_proof,
            merkle_root,
            signature,
        );

        // Should produce valid ABI-encoded calldata
        assert!(!calldata.is_empty());
        // First 4 bytes are the function selector
        assert!(calldata.len() > 4);
    }
}
