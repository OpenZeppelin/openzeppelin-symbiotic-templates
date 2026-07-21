// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ExtraArgsCodec} from "@chainlink/contracts-ccip/contracts/libraries/ExtraArgsCodec.sol";

/// @title CcipExtraArgs
/// @notice Shared GenericExtraArgsV3 encoder for CCIP v2 senders.
library CcipExtraArgs {
    bytes4 internal constant GENERIC_EXTRA_ARGS_V3_TAG = ExtraArgsCodec.GENERIC_EXTRA_ARGS_V3_TAG;

    /// @dev Encodes GenericExtraArgsV3 with: a single required CCV, no optional CCVs,
    /// requested finality = 0 (default wait-for-finality), no token transfer. Pass
    /// `executor == address(0)` to omit the executor field (OnRamp uses its default executor).
    function encodeWithCcv(address ccv, address executor, uint32 gasLimit) internal pure returns (bytes memory) {
        address[] memory ccvs = new address[](1);
        ccvs[0] = ccv;
        bytes[] memory ccvArgs = new bytes[](1);
        ccvArgs[0] = "";

        return ExtraArgsCodec._encodeGenericExtraArgsV3(
            ExtraArgsCodec.GenericExtraArgsV3({
                gasLimit: gasLimit,
                requestedFinalityConfig: bytes4(0),
                ccvs: ccvs,
                ccvArgs: ccvArgs,
                executor: executor,
                executorArgs: "",
                tokenReceiver: "",
                tokenArgs: ""
            })
        );
    }
}
