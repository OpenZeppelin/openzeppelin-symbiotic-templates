// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";
import {SlimEndpointV2, SlimSendUln302} from "../../../src/layerzero/mocks/SlimLayerZeroEndpoint.sol";
import {SymbioticLayerZeroDVN} from "../../../src/layerzero/SymbioticLayerZeroDVN.sol";
import {ExampleOApp} from "../../../src/layerzero/ExampleOApp.sol";
import {
    MessagingFee,
    MessagingReceipt
} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import {SetDefaultUlnConfigParam, UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";

/// @notice End-to-end harness coverage for the path `xtask msg send` exercises:
///         ExampleOApp.send -> SlimEndpointV2.send -> SlimSendUln302.slimSendPacket ->
///         ILayerZeroDVN.assignJob.
contract SlimLayerZeroEndpointTest is Test {
    uint32 constant SOURCE_EID = 31_337;
    uint32 constant DEST_EID = 31_338;
    uint256 constant DVN_BASE_FEE = 0.001 ether;

    SlimEndpointV2 endpoint;
    SlimSendUln302 sendUln;
    SymbioticLayerZeroDVN dvn;
    ExampleOApp oapp;

    address owner;
    address user;

    function setUp() public {
        owner = address(this);
        user = makeAddr("user");
        vm.deal(user, 1 ether);

        endpoint = new SlimEndpointV2(SOURCE_EID, owner);
        sendUln = new SlimSendUln302(payable(address(0)), address(endpoint), 0, 0);
        dvn = new SymbioticLayerZeroDVN(address(0), address(sendUln), address(0), SOURCE_EID, DVN_BASE_FEE);

        endpoint.registerLibrary(address(sendUln));
        endpoint.setDefaultSendLibrary(DEST_EID, address(sendUln));

        address[] memory required = new address[](1);
        required[0] = address(dvn);
        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: DEST_EID,
            config: UlnConfig({
                confirmations: 1,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: required,
                optionalDVNs: new address[](0)
            })
        });
        sendUln.setDefaultUlnConfigs(ulnParams);

        oapp = new ExampleOApp(address(endpoint), owner);
        oapp.setPeer(DEST_EID, bytes32(uint256(uint160(makeAddr("dstOApp")))));
    }

    function test_quote_returnsDvnFee() public view {
        bytes memory options = oapp.buildOptions(200_000);
        MessagingFee memory fee = oapp.quote(DEST_EID, "hello", options, false);
        assertEq(fee.nativeFee, DVN_BASE_FEE, "quote should equal DVN baseFee");
        assertEq(fee.lzTokenFee, 0);
    }

    function test_send_routesThroughDvn_andEmitsJobAssigned() public {
        bytes memory options = oapp.buildOptions(200_000);
        uint256 fee = oapp.quote(DEST_EID, "hello", options, false).nativeFee;

        vm.recordLogs();
        vm.prank(user);
        MessagingReceipt memory receipt = oapp.send{value: fee}(DEST_EID, "hello", options);

        assertEq(receipt.nonce, 1, "first send should have nonce 1");
        assertEq(receipt.fee.nativeFee, fee);
        assertGt(uint256(receipt.guid), 0);

        Vm.Log[] memory logs = vm.getRecordedLogs();
        bool foundJobAssigned;
        for (uint256 i; i < logs.length; i++) {
            if (
                logs[i].topics[0]
                    == keccak256(
                        "JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)"
                    )
            ) {
                foundJobAssigned = true;
                break;
            }
        }
        assertTrue(foundJobAssigned, "DVN.JobAssigned should be emitted");
    }

    function test_send_incrementsNoncePerChannel() public {
        bytes memory options = oapp.buildOptions(200_000);
        uint256 fee = oapp.quote(DEST_EID, "msg", options, false).nativeFee;

        vm.startPrank(user);
        MessagingReceipt memory r1 = oapp.send{value: fee}(DEST_EID, "msg1", options);
        MessagingReceipt memory r2 = oapp.send{value: fee}(DEST_EID, "msg2", options);
        vm.stopPrank();

        assertEq(r1.nonce, 1);
        assertEq(r2.nonce, 2);
        assertTrue(r1.guid != r2.guid, "guids should differ across nonces");
    }

    function test_quote_revertsWithoutSendLibrary() public {
        oapp.setPeer(99_999, bytes32(uint256(uint160(makeAddr("other")))));
        bytes memory options = oapp.buildOptions(200_000);
        vm.expectRevert(abi.encodeWithSelector(SlimEndpointV2.NoSendLibrary.selector, uint32(99_999)));
        oapp.quote(99_999, "msg", options, false);
    }
}
