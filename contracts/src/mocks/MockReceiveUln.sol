// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IReceiveUlnE2} from "../interfaces/IReceiveUlnE2.sol";

/// @title MockReceiveUln
/// @notice Mock ReceiveUln302 for testing DVN proof verification
/// @dev Accepts verify() calls from DVN and tracks verified packets
contract MockReceiveUln is IReceiveUlnE2 {
    /// @notice Verification record
    struct Verification {
        bytes32 payloadHash;
        uint64 confirmations;
        uint256 timestamp;
    }

    /// @notice Mapping from packet header hash to verification
    mapping(bytes32 => Verification) public verifications;

    /// @notice Count of total verifications
    uint256 public verificationCount;

    /// @notice Emitted when a packet is verified
    event PacketVerified(
        bytes32 indexed headerHash,
        bytes32 payloadHash,
        uint64 confirmations
    );

    /// @notice Emitted when verification is committed
    event VerificationCommitted(
        bytes32 indexed headerHash,
        bytes32 payloadHash
    );

    /// @notice Called by DVN to verify a packet
    /// @dev Implements IReceiveUlnE2.verify
    function verify(
        bytes calldata _packetHeader,
        bytes32 _payloadHash,
        uint64 _confirmations
    ) external override {
        bytes32 headerHash = keccak256(_packetHeader);

        verifications[headerHash] = Verification({
            payloadHash: _payloadHash,
            confirmations: _confirmations,
            timestamp: block.timestamp
        });

        verificationCount++;

        emit PacketVerified(headerHash, _payloadHash, _confirmations);
    }

    /// @notice Commit verification (for completeness)
    /// @dev Implements IReceiveUlnE2.commitVerification
    function commitVerification(
        bytes calldata _packetHeader,
        bytes32 _payloadHash
    ) external override {
        bytes32 headerHash = keccak256(_packetHeader);
        require(verifications[headerHash].timestamp > 0, "Not verified");

        emit VerificationCommitted(headerHash, _payloadHash);
    }

    /// @notice Check if a packet has been verified
    function isVerified(bytes32 headerHash) external view returns (bool) {
        return verifications[headerHash].timestamp > 0;
    }

    /// @notice Get verification details
    function getVerification(bytes32 headerHash)
        external
        view
        returns (bytes32 payloadHash, uint64 confirmations, uint256 timestamp)
    {
        Verification memory v = verifications[headerHash];
        return (v.payloadHash, v.confirmations, v.timestamp);
    }
}
