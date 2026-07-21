// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/// @title CcipExtraArgs
/// @notice Shared GenericExtraArgsV3 encoder for CCIP v2 senders.
library CcipExtraArgs {
    bytes4 internal constant GENERIC_EXTRA_ARGS_V3_TAG = 0xa69dd4aa;

    /// @dev Encodes GenericExtraArgsV3 with: a single required CCV, no optional CCVs,
    /// requested finality = 0 (default wait-for-finality), no token transfer. Pass
    /// `executor == address(0)` to omit the executor field (OnRamp uses its default executor).
    /// Layout:
    ///   tag(4) | gasLimit(4) | requestedFinalityConfig(4) | ccvsLength(1) |
    ///   ccvAddrLength(1) | ccvAddr(20) | ccvArgsLength(2) |
    ///   executorLength(1) | executor(0 or 20) | executorArgsLength(2) |
    ///   tokenReceiverLength(1) | tokenArgsLength(2)
    function encodeWithCcv(address ccv, address executor, uint32 gasLimit) internal pure returns (bytes memory) {
        bytes memory executorField = executor == address(0)
            ? abi.encodePacked(uint8(0))
            : abi.encodePacked(uint8(20), bytes20(executor));

        return abi.encodePacked(
            GENERIC_EXTRA_ARGS_V3_TAG,
            gasLimit,
            bytes4(0),
            uint8(1),
            uint8(20),
            bytes20(ccv),
            uint16(0),
            executorField,
            uint16(0),
            uint8(0),
            uint16(0)
        );
    }
}
