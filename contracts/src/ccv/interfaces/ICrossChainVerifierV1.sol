// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {Client} from "../libraries/Client.sol";
import {MessageV1Codec} from "../libraries/MessageV1Codec.sol";

interface ICrossChainVerifierV1 is IERC165 {
    function verifyMessage(
        MessageV1Codec.MessageV1 memory message,
        bytes32 messageId,
        bytes memory verifierResults
    ) external;

    function getFee(
        uint64 destChainSelector,
        Client.EVM2AnyMessage memory message,
        bytes memory extraArgs,
        uint16 blockConfirmations
    ) external view returns (uint16 feeUSDCents, uint32 gasForVerification, uint32 payloadSizeBytes);

    function forwardToVerifier(
        MessageV1Codec.MessageV1 calldata message,
        bytes32 messageId,
        address feeToken,
        uint256 feeTokenAmount,
        bytes calldata verifierArgs
    ) external returns (bytes memory verifierData);

    function getStorageLocations() external view returns (string[] memory);
}
