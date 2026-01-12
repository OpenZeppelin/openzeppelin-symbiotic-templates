// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "../interfaces/ISettlement.sol";

/// @title MockSettlement
/// @notice Mock Settlement contract for testing DVN destination chain
/// @dev Always returns true for signature verification (testing only!)
contract MockSettlement is ISettlement {
    // Default values for testing
    uint8 public constant DEFAULT_KEY_TAG = 15; // BLS-BN254
    uint256 public constant DEFAULT_QUORUM_THRESHOLD = 6600; // 66%

    /// @notice Always returns true for testing
    function verifyQuorumSigAt(
        bytes memory, /* message */
        uint8, /* keyTag */
        uint256, /* quorumThreshold */
        bytes calldata, /* proof */
        uint48, /* epoch */
        bytes memory /* hint */
    ) external pure override returns (bool) {
        return true;
    }

    /// @notice Returns default key tag (15 for BLS-BN254)
    function getRequiredKeyTagFromValSetHeaderAt(uint48 /* epoch */) external pure override returns (uint8) {
        return DEFAULT_KEY_TAG;
    }

    /// @notice Returns default quorum threshold (66%)
    function getQuorumThresholdFromValSetHeaderAt(uint48 /* epoch */) external pure override returns (uint256) {
        return DEFAULT_QUORUM_THRESHOLD;
    }

    /// @notice Returns current timestamp as capture time (always valid)
    function getCaptureTimestampFromValSetHeaderAt(uint48 /* epoch */) external view override returns (uint48) {
        return uint48(block.timestamp);
    }
}
