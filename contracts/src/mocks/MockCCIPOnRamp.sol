// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/// @title MockCCIPOnRamp
/// @notice Minimal source-side CCIP OnRamp mock for local devnet event emission.
contract MockCCIPOnRamp {
    struct Receipt {
        address issuer;
        uint32 destGasLimit;
        uint32 destBytesOverhead;
        uint256 feeTokenAmount;
        bytes extraArgs;
    }

    event CCIPMessageSent(
        uint64 indexed destChainSelector,
        address indexed sender,
        bytes32 indexed messageId,
        address feeToken,
        uint256 tokenAmountBeforeTokenPoolFees,
        bytes encodedMessage,
        Receipt[] receipts,
        bytes[] verifierBlobs
    );

    uint64 public nonce;

    /// @notice Emit a CCIPMessageSent event with a single version-tag verifier blob.
    function sendMessage(
        uint64 destChainSelector,
        bytes calldata encodedMessage,
        bytes4 versionTag
    ) external returns (bytes32 messageId) {
        nonce += 1;

        // Make message IDs deterministic from the emitted wire payload so destination mocks
        // can recompute the same ID during execute() verification.
        bytes memory wireMessage = abi.encode(nonce, msg.sender, encodedMessage);
        messageId = keccak256(wireMessage);

        Receipt[] memory receipts = new Receipt[](0);
        bytes[] memory verifierBlobs = new bytes[](1);
        verifierBlobs[0] = abi.encodePacked(versionTag);

        emit CCIPMessageSent(
            destChainSelector,
            msg.sender,
            messageId,
            address(0),
            0,
            wireMessage,
            receipts,
            verifierBlobs
        );
    }
}
