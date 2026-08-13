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
        return encodeWithCcvs(ccvs, executor, gasLimit);
    }

    /// @dev Encodes GenericExtraArgsV3 with an arbitrary CCV list (empty per-CCV args).
    /// An `address(0)` entry is a placeholder the OnRamp expands to the lane's default
    /// CCVs while keeping the explicit entries; see `encodeWithDefaultsAndCcv`.
    /// NOTE: requesting multiple CCVs at source is not enough on its own — every
    /// destination-required CCV must produce a verifier result at execution time, and
    /// the operator bundled with this template submits only the Symbiotic CCV's result
    /// to `OffRamp.execute`. Compose additional CCVs via destination policy (receiver /
    /// pool-required lists) only with an executor that can supply all of their results.
    function encodeWithCcvs(
        address[] memory ccvs,
        address executor,
        uint32 gasLimit
    ) internal pure returns (bytes memory) {
        bytes[] memory ccvArgs = new bytes[](ccvs.length);

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

    /// @dev Encodes `[address(0), ccv]`: the lane's default CCVs plus one explicit CCV.
    function encodeWithDefaultsAndCcv(
        address ccv,
        address executor,
        uint32 gasLimit
    ) internal pure returns (bytes memory) {
        address[] memory ccvs = new address[](2);
        ccvs[1] = ccv;
        return encodeWithCcvs(ccvs, executor, gasLimit);
    }
}
