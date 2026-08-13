// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {ExtraArgsCodec} from "@chainlink/contracts-ccip/contracts/libraries/ExtraArgsCodec.sol";

import {CcipExtraArgs} from "../../src/chainlink/CcipExtraArgs.sol";

/// @notice Round-trips CcipExtraArgs encodings through Chainlink's decoder. The
/// decoder takes calldata, so the harness re-enters itself via external calls.
contract CcipExtraArgsTest is Test {
    address internal constant RESOLVER_A = address(0xA11CE);
    address internal constant RESOLVER_B = address(0xB0B);
    address internal constant EXECUTOR = address(0xE7ec);

    function decode(bytes calldata encoded)
        external
        pure
        returns (ExtraArgsCodec.GenericExtraArgsV3 memory)
    {
        return ExtraArgsCodec._decodeGenericExtraArgsV3(encoded);
    }

    function _roundTrip(bytes memory encoded)
        internal
        view
        returns (ExtraArgsCodec.GenericExtraArgsV3 memory)
    {
        assertEq(bytes4(encoded), CcipExtraArgs.GENERIC_EXTRA_ARGS_V3_TAG);
        return this.decode(encoded);
    }

    function test_encodeWithCcv_singleEntry() public view {
        ExtraArgsCodec.GenericExtraArgsV3 memory args =
            _roundTrip(CcipExtraArgs.encodeWithCcv(RESOLVER_A, EXECUTOR, 200_000));

        assertEq(args.ccvs.length, 1);
        assertEq(args.ccvs[0], RESOLVER_A);
        assertEq(args.ccvArgs.length, 1);
        assertEq(args.ccvArgs[0], "");
        assertEq(args.executor, EXECUTOR);
        assertEq(args.gasLimit, 200_000);
    }

    function test_encodeWithCcvs_twoExplicitResolvers() public view {
        address[] memory ccvs = new address[](2);
        ccvs[0] = RESOLVER_A;
        ccvs[1] = RESOLVER_B;

        ExtraArgsCodec.GenericExtraArgsV3 memory args =
            _roundTrip(CcipExtraArgs.encodeWithCcvs(ccvs, EXECUTOR, 300_000));

        assertEq(args.ccvs.length, 2);
        assertEq(args.ccvs[0], RESOLVER_A);
        assertEq(args.ccvs[1], RESOLVER_B);
        assertEq(args.ccvArgs.length, 2);
        assertEq(args.executor, EXECUTOR);
        assertEq(args.gasLimit, 300_000);
    }

    function test_encodeWithDefaultsAndCcv_zeroPlaceholderSurvives() public view {
        ExtraArgsCodec.GenericExtraArgsV3 memory args =
            _roundTrip(CcipExtraArgs.encodeWithDefaultsAndCcv(RESOLVER_A, EXECUTOR, 200_000));

        // address(0) is the OnRamp's "append lane defaults" placeholder; it must
        // survive the encode/decode round-trip alongside the explicit resolver.
        assertEq(args.ccvs.length, 2);
        assertEq(args.ccvs[0], address(0));
        assertEq(args.ccvs[1], RESOLVER_A);
        assertEq(args.ccvArgs.length, 2);
    }

    function test_encodeWithCcvs_emptyListRequestsLaneDefaults() public view {
        ExtraArgsCodec.GenericExtraArgsV3 memory args =
            _roundTrip(CcipExtraArgs.encodeWithCcvs(new address[](0), EXECUTOR, 200_000));

        assertEq(args.ccvs.length, 0);
        assertEq(args.ccvArgs.length, 0);
    }
}
