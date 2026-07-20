// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ICrossChainVerifierV1} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierV1.sol";
import {ICrossChainVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierResolver.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";

/// @title MockCCIPOffRamp
/// @notice Minimal destination-side OffRamp mock that executes verifier checks.
contract MockCCIPOffRamp {
    error LengthMismatch();
    error VerifierNotConfigured(uint256 index);

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
        MessageV1Codec.MessageV1 memory message = MessageV1Codec._decodeMessageV1(encodedMessage);

        for (uint256 i = 0; i < ccvs.length; ++i) {
            address implementation =
                ICrossChainVerifierResolver(ccvs[i]).getInboundImplementation(verifierResults[i]);
            if (implementation == address(0)) {
                revert VerifierNotConfigured(i);
            }
            ICrossChainVerifierV1(implementation).verifyMessage(message, messageId, verifierResults[i]);
        }

        emit MessageExecuted(messageId, ccvs.length, verifierResults.length);
    }
}
