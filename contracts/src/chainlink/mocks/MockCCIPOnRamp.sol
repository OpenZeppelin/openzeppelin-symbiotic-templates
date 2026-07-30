// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ICrossChainVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierResolver.sol";
import {ICrossChainVerifierV1} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierV1.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";

/// @title MockCCIPOnRamp
/// @notice Minimal source-side CCIP OnRamp mock for local devnet event emission.
contract MockCCIPOnRamp {
    error InvalidVersionTag(bytes4 expected, bytes4 actual);
    error VerifierNotConfigured(uint64 destChainSelector);

    /// @dev Mirrors OnRamp.sol's in-contract `Receipt` struct / `CCIPMessageSent` event
    /// declarations (not importable without inheriting the full OnRamp). If this shape
    /// ever changes, update in lockstep with xtask/src/msg.rs (sol! MockCcipReceipt /
    /// CCIPMessageSent decode) and the operator's chainlink_ccv message decoding.
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
    ICrossChainVerifierResolver public immutable resolver;

    constructor(address resolverAddress) {
        resolver = ICrossChainVerifierResolver(resolverAddress);
    }

    /// @notice Emit a CCIPMessageSent event with a single version-tag verifier blob.
    function sendMessage(
        uint64 destChainSelector,
        bytes calldata encodedMessage,
        bytes4 versionTag,
        address executor
    ) external returns (bytes32 messageId) {
        nonce += 1;

        // `encodedMessage` is already a CCIP MessageV1 wire blob (the verifier decodes it
        // directly, e.g. for source-finality). Emit it as-is and derive the messageId from
        // it so the destination OffRamp mock recomputes the same keccak256 during execute().
        // Per-send uniqueness comes from the MessageV1 sequence number set by the sender.
        bytes memory wireMessage = encodedMessage;
        messageId = keccak256(wireMessage);

        address implementation = resolver.getOutboundImplementation(destChainSelector, "");
        if (implementation == address(0)) {
            revert VerifierNotConfigured(destChainSelector);
        }
        MessageV1Codec.MessageV1 memory message = MessageV1Codec._decodeMessageV1(encodedMessage);
        bytes memory verifierBlob = ICrossChainVerifierV1(implementation).forwardToVerifier(
            message, messageId, address(0), 0, ""
        );
        bytes4 actualVersionTag = bytes4(verifierBlob);
        if (actualVersionTag != versionTag) {
            revert InvalidVersionTag(versionTag, actualVersionTag);
        }

        // Receipt layout the verifier expects: [CCV..., Token?, Executor, NetworkFee].
        // One verifier blob => one CCV, no token transfer, so [CCV, Executor, NetworkFee].
        // The Executor (second-to-last) is settable so tests can target a specific
        // operator's self-executor and exercise per-message executor gating.
        Receipt[] memory receipts = new Receipt[](3);
        receipts[0] = Receipt(msg.sender, 0, 0, 0, ""); // CCV
        receipts[1] = Receipt(executor, 0, 0, 0, ""); // Executor
        receipts[2] = Receipt(msg.sender, 0, 0, 0, ""); // NetworkFee
        bytes[] memory verifierBlobs = new bytes[](1);
        verifierBlobs[0] = verifierBlob;

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
