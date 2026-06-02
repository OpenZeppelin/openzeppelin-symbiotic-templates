// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ICrossChainVerifierV1} from "../ccv/interfaces/ICrossChainVerifierV1.sol";
import {MessageV1Codec} from "../ccv/libraries/MessageV1Codec.sol";

/// @title MockCCIPOffRamp
/// @notice Minimal destination-side OffRamp mock that executes verifier checks.
contract MockCCIPOffRamp {
    error LengthMismatch();

    uint64 public immutable sourceChainSelector;

    event MessageExecuted(bytes32 indexed messageId, uint256 ccvCount, uint256 verifierResultCount);

    constructor(uint64 sourceChainSelector_) {
        sourceChainSelector = sourceChainSelector_;
    }

    function execute(
        bytes calldata encodedMessage,
        address[] calldata ccvs,
        bytes[] calldata verifierResults,
        uint32
    ) external {
        if (ccvs.length != verifierResults.length) {
            revert LengthMismatch();
        }

        bytes32 messageId = keccak256(encodedMessage);
        MessageV1Codec.MessageV1 memory message = MessageV1Codec.MessageV1({
            sourceChainSelector: sourceChainSelector,
            destChainSelector: uint64(block.chainid),
            messageNumber: 0,
            executionGasLimit: 0,
            ccipReceiveGasLimit: 0,
            finality: bytes4(0),
            ccvAndExecutorHash: bytes32(0),
            onRampAddress: new bytes(0),
            offRampAddress: abi.encodePacked(address(this)),
            sender: new bytes(0),
            receiver: new bytes(0),
            destBlob: new bytes(0),
            tokenTransfer: new MessageV1Codec.TokenTransferV1[](0),
            data: encodedMessage
        });

        for (uint256 i = 0; i < ccvs.length; ++i) {
            ICrossChainVerifierV1(ccvs[i]).verifyMessage(message, messageId, verifierResults[i]);
        }

        emit MessageExecuted(messageId, ccvs.length, verifierResults.length);
    }
}
