// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "../../src/interfaces/ISettlement.sol";

/// @title AssertingSettlement
/// @notice A view-safe test helper that validates DVN calls Settlement with correct parameters
/// @dev Used in tests to verify the DVN passes the expected values to Settlement
contract AssertingSettlement is ISettlement {
    // ============ Storage for expectations ============

    /// @notice Expected epoch value for verifyQuorumSigAt
    uint48 public expectedEpoch;

    /// @notice Expected message hash - the keccak256(abi.encode(chainid, dvnAddr, merkleRoot))
    bytes32 public expectedMessageHash;

    /// @notice Expected proof hash - keccak256 of the BLS signature portion (signature[6:])
    bytes32 public expectedProofHash;

    /// @notice What verifyQuorumSigAt should return
    bool public verifyReturnValue;

    /// @notice If true, verifyQuorumSigAt will revert
    bool public shouldRevertOnVerify;

    /// @notice If true, any call to Settlement will revert (for cached-root tests)
    bool public shouldRevertOnAnyCall;

    // ============ Per-epoch config maps ============

    /// @notice Capture timestamp for each epoch
    mapping(uint48 => uint48) public captureTimestampAt;

    /// @notice Key tag for each epoch
    mapping(uint48 => uint8) public keyTagAt;

    /// @notice Quorum threshold for each epoch
    mapping(uint48 => uint256) public quorumThresholdAt;

    // ============ Setter functions (non-view) ============

    /// @notice Set the expected epoch for verifyQuorumSigAt
    /// @param epoch The expected epoch value
    function setExpectedEpoch(uint48 epoch) external {
        expectedEpoch = epoch;
    }

    /// @notice Set the expected message hash
    /// @param hash The expected keccak256(abi.encode(chainid, dvnAddr, merkleRoot))
    function setExpectedMessageHash(bytes32 hash) external {
        expectedMessageHash = hash;
    }

    /// @notice Set the expected proof hash
    /// @param hash The expected keccak256 of the BLS signature portion
    function setExpectedProofHash(bytes32 hash) external {
        expectedProofHash = hash;
    }

    /// @notice Set the epoch configuration
    /// @param epoch The epoch to configure
    /// @param captureTs The capture timestamp for this epoch
    /// @param keyTag The key tag for this epoch
    /// @param threshold The quorum threshold for this epoch
    function setEpochConfig(uint48 epoch, uint48 captureTs, uint8 keyTag, uint256 threshold) external {
        captureTimestampAt[epoch] = captureTs;
        keyTagAt[epoch] = keyTag;
        quorumThresholdAt[epoch] = threshold;
    }

    /// @notice Set the return value for verifyQuorumSigAt
    /// @param value The value to return
    function setVerifyReturnValue(bool value) external {
        verifyReturnValue = value;
    }

    /// @notice Set whether verifyQuorumSigAt should revert
    /// @param value If true, verifyQuorumSigAt will revert
    function setShouldRevertOnVerify(bool value) external {
        shouldRevertOnVerify = value;
    }

    /// @notice Set whether any Settlement call should revert
    /// @param value If true, all Settlement calls will revert
    function setShouldRevertOnAnyCall(bool value) external {
        shouldRevertOnAnyCall = value;
    }

    // ============ ISettlement view functions ============

    /// @inheritdoc ISettlement
    function getCaptureTimestampFromValSetHeaderAt(uint48 epoch) external view override returns (uint48) {
        require(!shouldRevertOnAnyCall, "Settlement should not be called");
        return captureTimestampAt[epoch];
    }

    /// @inheritdoc ISettlement
    function getRequiredKeyTagFromValSetHeaderAt(uint48 epoch) external view override returns (uint8) {
        require(!shouldRevertOnAnyCall, "Settlement should not be called");
        return keyTagAt[epoch];
    }

    /// @inheritdoc ISettlement
    function getQuorumThresholdFromValSetHeaderAt(uint48 epoch) external view override returns (uint256) {
        require(!shouldRevertOnAnyCall, "Settlement should not be called");
        return quorumThresholdAt[epoch];
    }

    /// @inheritdoc ISettlement
    function verifyQuorumSigAt(
        bytes memory message,
        uint8 keyTag,
        uint256 threshold,
        bytes calldata proof,
        uint48 epoch,
        bytes memory hint
    ) external view override returns (bool) {
        require(!shouldRevertOnAnyCall, "Settlement should not be called");

        if (shouldRevertOnVerify) {
            revert("Settlement verification failed");
        }

        require(epoch == expectedEpoch, "Epoch mismatch");

        bytes32 msgHash = abi.decode(message, (bytes32));
        if (expectedMessageHash != bytes32(0)) {
            require(msgHash == expectedMessageHash, "Message hash mismatch");
        }

        if (expectedProofHash != bytes32(0)) {
            require(keccak256(proof) == expectedProofHash, "Proof hash mismatch");
        }

        require(keyTag == keyTagAt[epoch], "KeyTag mismatch");
        require(threshold == quorumThresholdAt[epoch], "Threshold mismatch");
        require(hint.length == 0, "Hint should be empty");

        return verifyReturnValue;
    }
}
