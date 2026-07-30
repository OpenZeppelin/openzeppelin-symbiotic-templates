// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {VersionedVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/ccvs/VersionedVerifierResolver.sol";

import {DeployCCV} from "../script/DeployCCV.s.sol";

/// @notice Locks the governance calldata helpers to their on-chain ABI so a
/// Safe/timelock owner can rely on the printed (target, calldata) pairs.
contract DeployCCVCalldataTest is Test {
    DeployCCV internal script;

    address internal constant RESOLVER = address(0x1234);
    address internal constant VERIFIER = address(0x5678);
    bytes4 internal constant VERSION_TAG = 0x1a75bd93;

    function setUp() public {
        script = new DeployCCV();
    }

    function test_printAcceptOwnershipCall_matchesAbi() public view {
        (address target, bytes memory data) = script.printAcceptOwnershipCall(RESOLVER);
        assertEq(target, RESOLVER);
        assertEq(data, abi.encodeWithSignature("acceptOwnership()"));
    }

    function test_printRegisterVerifierCalls_matchesAbi() public view {
        uint64[] memory selectors = new uint64[](2);
        selectors[0] = 111;
        selectors[1] = 222;

        (address target, bytes memory inboundData, bytes memory outboundData) =
            script.printRegisterVerifierCalls(RESOLVER, VERSION_TAG, VERIFIER, selectors);

        assertEq(target, RESOLVER);
        assertEq(
            bytes4(inboundData),
            bytes4(keccak256("applyInboundImplementationUpdates((bytes4,address)[])"))
        );
        assertEq(
            bytes4(outboundData),
            bytes4(keccak256("applyOutboundImplementationUpdates((uint64,address)[])"))
        );

        VersionedVerifierResolver.InboundImplementationArgs[] memory inbound = abi.decode(
            _stripSelector(inboundData), (VersionedVerifierResolver.InboundImplementationArgs[])
        );
        assertEq(inbound.length, 1);
        assertEq(inbound[0].version, VERSION_TAG);
        assertEq(inbound[0].verifier, VERIFIER);

        VersionedVerifierResolver.OutboundImplementationArgs[] memory outbound = abi.decode(
            _stripSelector(outboundData), (VersionedVerifierResolver.OutboundImplementationArgs[])
        );
        assertEq(outbound.length, 2);
        assertEq(outbound[0].destChainSelector, uint64(111));
        assertEq(outbound[1].destChainSelector, uint64(222));
        assertEq(outbound[0].verifier, VERIFIER);
    }

    function test_printSetEpochValidityCall_matchesAbi() public view {
        (address target, bytes memory data) = script.printSetEpochValidityCall(VERIFIER, 24 hours);
        assertEq(target, VERIFIER);
        assertEq(data, abi.encodeWithSignature("setEpochValidity(uint256)", 24 hours));
    }

    function _stripSelector(bytes memory data) internal pure returns (bytes memory args) {
        args = new bytes(data.length - 4);
        for (uint256 i = 0; i < args.length; ++i) {
            args[i] = data[i + 4];
        }
    }
}
