use alloy::primitives::{keccak256, Address, B256, U256};

/// Compute DVN-compatible leaf hash for LayerZero proof submission
/// Matches Solidity: keccak256(abi.encodePacked(keccak256(packetHeader), payloadHash, confirmations))
/// Note: abi.encodePacked(uint64) uses big-endian, same as to_be_bytes()
pub fn compute_dvn_leaf(packet_header: &[u8], payload_hash: B256, confirmations: u64) -> B256 {
    let header_hash = keccak256(packet_header);
    // abi.encodePacked: 32 bytes + 32 bytes + 8 bytes = 72 bytes total
    let mut packed = Vec::with_capacity(72);
    packed.extend_from_slice(header_hash.as_slice()); // 32 bytes
    packed.extend_from_slice(payload_hash.as_slice()); // 32 bytes
    packed.extend_from_slice(&confirmations.to_be_bytes()); // 8 bytes (big-endian)
    keccak256(&packed)
}

/// Compute domain-separated signing hash for BLS signature
/// Matches Solidity: keccak256(abi.encode(block.chainid, address(this), merkleRoot))
///
/// This ensures signatures are bound to a specific chain and DVN contract,
/// preventing replay attacks across chains or contracts.
pub fn compute_signing_hash(chain_id: u64, dvn_address: Address, merkle_root: B256) -> B256 {
    // abi.encode pads each value to 32 bytes:
    // - uint256 chainId: 32 bytes
    // - address dvnAddress: 12 bytes padding + 20 bytes address = 32 bytes
    // - bytes32 merkleRoot: 32 bytes
    // Total: 96 bytes
    let mut encoded = Vec::with_capacity(96);

    // uint256 chainId (32 bytes, big-endian)
    encoded.extend_from_slice(&U256::from(chain_id).to_be_bytes::<32>());

    // address (20 bytes, left-padded to 32 bytes)
    encoded.extend_from_slice(&[0u8; 12]); // 12 bytes of zero padding
    encoded.extend_from_slice(dvn_address.as_slice()); // 20 bytes

    // bytes32 merkleRoot (32 bytes)
    encoded.extend_from_slice(merkle_root.as_slice());

    keccak256(&encoded)
}

/// Commutative hash - sorts siblings before hashing (OpenZeppelin compatible)
/// Ensures H(a, b) == H(b, a)
fn commutative_hash(a: B256, b: B256) -> B256 {
    let mut combined = [0u8; 64];
    if a <= b {
        combined[..32].copy_from_slice(a.as_slice());
        combined[32..].copy_from_slice(b.as_slice());
    } else {
        combined[..32].copy_from_slice(b.as_slice());
        combined[32..].copy_from_slice(a.as_slice());
    }
    keccak256(combined)
}

/// Compute Merkle root from pre-hashed leaves
/// - Leaves are NOT hashed again (DisableLeafHashing=true)
/// - Siblings are sorted before hashing (SortSiblingPairs=true)
pub fn merkle_root(leaves: &[B256]) -> Option<B256> {
    match leaves.len() {
        0 => None,
        1 => Some(leaves[0]),
        _ => {
            let mut current: Vec<B256> = leaves.to_vec();

            while current.len() > 1 {
                // Handle odd number of nodes by duplicating last one (matches Go)
                if current.len() % 2 == 1 {
                    current.push(*current.last().expect("non-empty vec in merkle loop"));
                }

                let mut next = Vec::with_capacity(current.len() / 2);
                for chunk in current.chunks(2) {
                    // Now guaranteed to have pairs
                    next.push(commutative_hash(chunk[0], chunk[1]));
                }
                current = next;
            }

            Some(current[0])
        }
    }
}

/// Merkle proof with path index for verification
#[derive(Debug, Clone)]
pub struct MerkleProof {
    pub leaf: B256,
    pub siblings: Vec<B256>,
    pub path: u32, // Bit-encoded path (GeneralIndex in Go)
}

/// Generate proof for a specific leaf
/// Leaves MUST be sorted lexicographically
pub fn generate_proof(sorted_leaves: &[B256], leaf: B256) -> Option<MerkleProof> {
    if sorted_leaves.is_empty() {
        return None;
    }

    // Find the leaf index using binary search (leaves are sorted)
    let leaf_idx = sorted_leaves
        .binary_search_by(|probe| probe.as_slice().cmp(leaf.as_slice()))
        .ok()?;

    let mut siblings = Vec::new();
    let mut path: u32 = 0;
    let mut current = sorted_leaves.to_vec();
    let mut idx = leaf_idx;
    let mut level = 0;

    while current.len() > 1 {
        // Handle odd number of nodes by duplicating last one (matches Go)
        if current.len() % 2 == 1 {
            current.push(*current.last().expect("non-empty vec in merkle loop"));
        }

        let mut next = Vec::with_capacity(current.len() / 2);

        for (i, chunk) in current.chunks(2).enumerate() {
            // Check if this chunk contains our node
            if i == idx / 2 {
                // Determine sibling and path bit (Go convention: LEFT=1, RIGHT=0)
                if idx % 2 == 0 {
                    // Our node is on the left, sibling is on the right
                    siblings.push(chunk[1]);
                    path |= 1 << level; // LEFT sets bit (Go convention)
                } else {
                    // Our node is on the right, sibling is on the left
                    siblings.push(chunk[0]);
                    // path bit stays 0 (right) - Go convention
                }
            }
            next.push(commutative_hash(chunk[0], chunk[1]));
        }

        current = next;
        idx /= 2;
        level += 1;
    }

    Some(MerkleProof {
        leaf,
        siblings,
        path,
    })
}

/// Verify a Merkle proof
pub fn verify_proof(proof: &MerkleProof, root: B256) -> bool {
    let mut current = proof.leaf;

    for (i, sibling) in proof.siblings.iter().enumerate() {
        current = commutative_hash(current, *sibling);
        // Note: with commutative hash, the path bits don't affect the result
        // since commutative_hash always sorts before hashing
        let _ = (proof.path >> i) & 1; // Path bit not used due to commutative hash
    }

    current == root
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Helper function to sort leaves for merkle tree tests
    fn sort_leaves(leaves: &[B256]) -> Vec<B256> {
        let unique: HashSet<B256> = leaves.iter().copied().collect();
        let mut sorted: Vec<B256> = unique.into_iter().collect();
        sorted.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        sorted
    }

    #[test]
    fn test_merkle_root_two_leaves() {
        // Test Vector 1 from spec
        let leaf_0 = B256::from_slice(&{
            let mut arr = [0u8; 32];
            arr[31] = 1;
            arr
        });
        let leaf_1 = B256::from_slice(&{
            let mut arr = [0u8; 32];
            arr[31] = 2;
            arr
        });

        let root = merkle_root(&[leaf_0, leaf_1]).unwrap();

        // Expected: keccak256(leaf_0 || leaf_1) since leaf_0 < leaf_1
        let expected = B256::from_slice(
            &hex::decode("e90b7bceb6e7df5418fb78d8ee546e97c83a08bbccc01a0644d599ccd2a7c2e0")
                .unwrap(),
        );
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_single_leaf_with_zero() {
        // Test Vector 2: Single leaf with zero padding
        let leaf_0 = B256::from_slice(&[0xaa; 32]);
        let leaf_1 = B256::ZERO;

        // Sort: leaf_1 (zero) < leaf_0 (0xaa...)
        let sorted = sort_leaves(&[leaf_0, leaf_1]);
        let root = merkle_root(&sorted).unwrap();

        // Expected: keccak256(0x00...00 || 0xaa...aa)
        // Verified with: cast keccak "0x0000000000000000000000000000000000000000000000000000000000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let expected = B256::from_slice(
            &hex::decode("e60eb41b492e32a28ec6d7ba5db66f1206fcdf79cf8042e94129c92deaf6ffbe")
                .unwrap(),
        );
        assert_eq!(root, expected);
    }

    #[test]
    fn test_proof_generation_and_verification() {
        let leaves = vec![
            B256::from_slice(&[1u8; 32]),
            B256::from_slice(&[2u8; 32]),
            B256::from_slice(&[3u8; 32]),
            B256::from_slice(&[4u8; 32]),
        ];

        let sorted = sort_leaves(&leaves);
        let root = merkle_root(&sorted).unwrap();

        for leaf in &sorted {
            let proof = generate_proof(&sorted, *leaf).unwrap();
            assert!(verify_proof(&proof, root));
        }
    }

    // === Sibling count tests (matching Go tests) ===

    #[test]
    fn test_proof_one_sibling() {
        // 2 leaves = 1 sibling in proof
        let leaves = vec![keccak256("leaf1"), keccak256("leaf2")];
        let sorted = sort_leaves(&leaves);

        // Test first leaf
        let proof = generate_proof(&sorted, sorted[0]).unwrap();
        assert_eq!(proof.siblings.len(), 1, "expected 1 sibling for 2 leaves");
        assert_eq!(proof.leaf, sorted[0]);
        assert_eq!(
            proof.siblings[0], sorted[1],
            "sibling should be the other leaf"
        );

        // Test second leaf
        let proof = generate_proof(&sorted, sorted[1]).unwrap();
        assert_eq!(proof.siblings.len(), 1);
        assert_eq!(proof.siblings[0], sorted[0]);

        // Verify both proofs
        let root = merkle_root(&sorted).unwrap();
        for leaf in &sorted {
            let proof = generate_proof(&sorted, *leaf).unwrap();
            assert!(verify_proof(&proof, root));
        }
    }

    #[test]
    fn test_proof_two_siblings() {
        // 3-4 leaves = 2 siblings in proof
        let leaves: Vec<B256> = (1..=4).map(|i| keccak256(format!("leaf{}", i))).collect();
        let sorted = sort_leaves(&leaves);

        for leaf in &sorted {
            let proof = generate_proof(&sorted, *leaf).unwrap();
            assert_eq!(proof.siblings.len(), 2, "expected 2 siblings for 4 leaves");

            let root = merkle_root(&sorted).unwrap();
            assert!(verify_proof(&proof, root));
        }
    }

    #[test]
    fn test_proof_three_siblings() {
        // 5-8 leaves = 3 siblings in proof
        let leaves: Vec<B256> = (1..=8).map(|i| keccak256(format!("leaf{}", i))).collect();
        let sorted = sort_leaves(&leaves);
        let root = merkle_root(&sorted).unwrap();

        for leaf in &sorted {
            let proof = generate_proof(&sorted, *leaf).unwrap();
            assert_eq!(proof.siblings.len(), 3, "expected 3 siblings for 8 leaves");
            assert!(verify_proof(&proof, root));
        }
    }

    #[test]
    fn test_proof_four_siblings() {
        // 9-16 leaves = 4 siblings in proof
        let leaves: Vec<B256> = (1..=16).map(|i| keccak256(format!("leaf{}", i))).collect();
        let sorted = sort_leaves(&leaves);
        let root = merkle_root(&sorted).unwrap();

        // Test various positions
        for idx in [0, 5, 10, 15] {
            let proof = generate_proof(&sorted, sorted[idx]).unwrap();
            assert_eq!(
                proof.siblings.len(),
                4,
                "expected 4 siblings for 16 leaves at index {}",
                idx
            );
            assert!(verify_proof(&proof, root));
        }
    }

    #[test]
    fn test_proof_not_found() {
        let leaves = vec![keccak256("leaf1"), keccak256("leaf2")];
        let sorted = sort_leaves(&leaves);

        let non_existent = keccak256("nonexistent");
        let result = generate_proof(&sorted, non_existent);

        assert!(result.is_none(), "expected None for non-existent leaf");
    }

    #[test]
    fn test_verify_proof_wrong_root() {
        let leaves = vec![keccak256("leaf1"), keccak256("leaf2")];
        let sorted = sort_leaves(&leaves);
        let proof = generate_proof(&sorted, sorted[0]).unwrap();

        let wrong_root = keccak256("wrong_root");
        assert!(
            !verify_proof(&proof, wrong_root),
            "proof should be invalid with wrong root"
        );
    }

    #[test]
    fn test_verify_proof_tampered_siblings() {
        let leaves: Vec<B256> = (1..=4).map(|i| keccak256(format!("leaf{}", i))).collect();
        let sorted = sort_leaves(&leaves);
        let root = merkle_root(&sorted).unwrap();

        let mut proof = generate_proof(&sorted, sorted[0]).unwrap();

        // Tamper with a sibling
        if !proof.siblings.is_empty() {
            proof.siblings[0] = keccak256("tampered");
        }

        assert!(
            !verify_proof(&proof, root),
            "proof should be invalid with tampered siblings"
        );
    }

    #[test]
    fn test_tree_root_deterministic() {
        let leaves = vec![keccak256("leaf1"), keccak256("leaf2")];
        let sorted = sort_leaves(&leaves);

        let root1 = merkle_root(&sorted).unwrap();
        let root2 = merkle_root(&sorted).unwrap();

        assert_eq!(root1, root2, "merkle root should be deterministic");
    }

    #[test]
    fn test_merkle_proof_structure() {
        let leaves: Vec<B256> = (1..=3).map(|i| keccak256(format!("leaf{}", i))).collect();
        let sorted = sort_leaves(&leaves);

        let proof = generate_proof(&sorted, sorted[0]).unwrap();

        // Verify structure
        assert_eq!(proof.leaf, sorted[0]);
        assert!(!proof.siblings.is_empty(), "expected non-empty siblings");
        // Path is the bit-encoded index, should be valid
    }

    #[test]
    fn test_compute_dvn_leaf() {
        // Test vector: known inputs should produce deterministic output
        // packetHeader: 81 bytes of 0x01
        let packet_header = vec![0x01u8; 81];
        // payloadHash: 32 bytes of 0x02
        let payload_hash = B256::from_slice(&[0x02u8; 32]);
        // confirmations: 15
        let confirmations: u64 = 15;

        let leaf = compute_dvn_leaf(&packet_header, payload_hash, confirmations);

        // Verify it's deterministic
        let leaf2 = compute_dvn_leaf(&packet_header, payload_hash, confirmations);
        assert_eq!(leaf, leaf2, "compute_dvn_leaf should be deterministic");

        // Verify changing confirmations changes the leaf
        let leaf3 = compute_dvn_leaf(&packet_header, payload_hash, 16);
        assert_ne!(leaf, leaf3, "different confirmations should produce different leaf");

        // Verify changing payload_hash changes the leaf
        let different_payload = B256::from_slice(&[0x03u8; 32]);
        let leaf4 = compute_dvn_leaf(&packet_header, different_payload, confirmations);
        assert_ne!(leaf, leaf4, "different payloadHash should produce different leaf");
    }

    #[test]
    fn test_end_to_end_proof_various_sizes() {
        // Test complete flow with different tree sizes
        for num_leaves in [2, 4, 8, 16] {
            let leaves: Vec<B256> = (0..num_leaves)
                .map(|i| keccak256(format!("data{}", i)))
                .collect();
            let sorted = sort_leaves(&leaves);
            let root = merkle_root(&sorted).unwrap();

            let expected_siblings = (num_leaves as f64).log2().ceil() as usize;

            for leaf in &sorted {
                let proof = generate_proof(&sorted, *leaf).unwrap();
                assert_eq!(
                    proof.siblings.len(),
                    expected_siblings,
                    "expected {} siblings for {} leaves",
                    expected_siblings,
                    num_leaves
                );
                assert!(verify_proof(&proof, root), "proof should be valid");
            }
        }
    }

    #[test]
    fn test_compute_signing_hash() {
        use super::compute_signing_hash;

        // Test vector matching Solidity:
        // keccak256(abi.encode(block.chainid, address(this), merkleRoot))
        //
        // Solidity abi.encode for (uint256, address, bytes32):
        // - uint256 chainId: 32 bytes, big-endian
        // - address: 12 bytes zero padding + 20 bytes address
        // - bytes32: 32 bytes as-is
        // Total: 96 bytes

        let chain_id: u64 = 31338;
        let dvn_address: Address = "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0"
            .parse()
            .unwrap();
        let merkle_root = B256::from_slice(&[0xaa; 32]);

        let hash = compute_signing_hash(chain_id, dvn_address, merkle_root);

        // Verify it's deterministic
        let hash2 = compute_signing_hash(chain_id, dvn_address, merkle_root);
        assert_eq!(hash, hash2, "compute_signing_hash should be deterministic");

        // Verify changing chain_id changes the hash
        let hash3 = compute_signing_hash(31337, dvn_address, merkle_root);
        assert_ne!(hash, hash3, "different chain_id should produce different hash");

        // Verify changing dvn_address changes the hash
        let different_address: Address = "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        let hash4 = compute_signing_hash(chain_id, different_address, merkle_root);
        assert_ne!(hash, hash4, "different dvn_address should produce different hash");

        // Verify changing merkle_root changes the hash
        let different_root = B256::from_slice(&[0xbb; 32]);
        let hash5 = compute_signing_hash(chain_id, dvn_address, different_root);
        assert_ne!(hash, hash5, "different merkle_root should produce different hash");

        // Verify the encoding is correct (96 bytes total)
        // We can verify this by manually computing:
        // abi.encode(31338, 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0, 0xaa...aa)
        //
        // chainId (31338 = 0x7a6a):
        // 0x0000000000000000000000000000000000000000000000000000000000007a6a
        // address (left-padded):
        // 0x0000000000000000000000009fE46736679d2D9a65F0992F2272dE9f3c7fa6e0
        // merkleRoot:
        // 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        //
        // Verified with: cast keccak $(cast abi-encode "f(uint256,address,bytes32)" 31338 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)
        let expected = B256::from_slice(
            &hex::decode("ef430600a751b734344328725354e20ea6a332f32eb9651fdf75ef4d70409c69")
                .unwrap(),
        );
        assert_eq!(hash, expected, "hash should match Solidity abi.encode output");
    }
}
