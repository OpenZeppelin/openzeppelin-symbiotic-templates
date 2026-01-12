// SPDX-License-Identifier: MIT
pragma solidity >=0.8.0;

/// @title ISettlement
/// @notice Minimal interface for Symbiotic Settlement contract
/// @dev Only includes the 4 functions used by DVN
interface ISettlement {
    /// @notice Verify a quorum signature at a specific epoch
    /// @param message The message that was signed
    /// @param keyTag The key tag for the signature scheme (e.g., 15 for BLS-BN254)
    /// @param quorumThreshold The quorum threshold to meet
    /// @param proof The aggregated BLS signature proof
    /// @param epoch The Symbiotic epoch
    /// @param hint Optional hint data (unused, pass empty bytes)
    /// @return True if the signature is valid and meets quorum
    function verifyQuorumSigAt(
        bytes memory message,
        uint8 keyTag,
        uint256 quorumThreshold,
        bytes calldata proof,
        uint48 epoch,
        bytes memory hint
    ) external view returns (bool);

    /// @notice Get the required key tag for a given epoch
    /// @param epoch The Symbiotic epoch
    /// @return The key tag (e.g., 15 for BLS-BN254)
    function getRequiredKeyTagFromValSetHeaderAt(uint48 epoch) external view returns (uint8);

    /// @notice Get the quorum threshold for a given epoch
    /// @param epoch The Symbiotic epoch
    /// @return The quorum threshold (e.g., 6600 for 66%)
    function getQuorumThresholdFromValSetHeaderAt(uint48 epoch) external view returns (uint256);

    /// @notice Get the capture timestamp for a given epoch
    /// @param epoch The Symbiotic epoch
    /// @return The timestamp when the epoch was captured
    function getCaptureTimestampFromValSetHeaderAt(uint48 epoch) external view returns (uint48);
}
