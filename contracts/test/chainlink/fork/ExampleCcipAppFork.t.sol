// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {Client} from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

import {ExampleCcipApp} from "../../../src/chainlink/ExampleCcipApp.sol";
import {CCVForkBase} from "./CCVForkBase.sol";


/// @notice Fork test that drives the full source-side flow through ExampleCcipApp
/// rather than calling Router.ccipSend directly. Validates that the unified
/// app contract correctly assembles extraArgs and pays the Router.
contract ExampleCcipAppForkTest is CCVForkBase {
    address constant ROUTER = 0x0Ec6D443B425982f1F2862Dd0ffBFD431FCb6b8b;

    uint64 constant SEPOLIA_SELECTOR = 16_015_286_601_757_825_753;

    ExampleCcipApp internal app;
    address internal operator;
    address internal user;

    function setUp() public {
        require(block.chainid == 84_532, "expected Base Sepolia fork (chainid 84532)");

        _deployVerifierAndResolver(vm.envAddress("SOURCE_CCIP_RMN_ADDRESS"), IRouter(ROUTER), SEPOLIA_SELECTOR);
        _registerOutbound(SEPOLIA_SELECTOR);

        operator = Client.NO_EXECUTION_ADDRESS;
        app = new ExampleCcipApp(ROUTER, address(resolver), operator);

        // Trust a remote app on Sepolia — we don't actually deploy it here,
        // just a stub address so send() proceeds.
        app.setRemoteApp(SEPOLIA_SELECTOR, makeAddr("remoteCcipApp"));

        user = makeAddr("ccvForkAppUser");
    }

    function testQuoteIsNonZero() public view {
        uint256 fee = app.quote(SEPOLIA_SELECTOR, "hello from app", 200_000);
        assertGt(fee, 0, "Router quoted zero fee");
    }

    function testSendThroughApp() public {
        uint256 fee = app.quote(SEPOLIA_SELECTOR, "hello from app", 200_000);

        vm.deal(user, fee * 2);
        vm.prank(user);
        bytes32 messageId = app.send{value: fee}(SEPOLIA_SELECTOR, "hello from app", 200_000);

        assertTrue(messageId != bytes32(0), "send returned zero messageId");
    }

    function testSendRefundsExcess() public {
        uint256 fee = app.quote(SEPOLIA_SELECTOR, "hello from app", 200_000);
        uint256 overpay = fee + 0.001 ether;

        vm.deal(user, overpay);
        vm.prank(user);
        app.send{value: overpay}(SEPOLIA_SELECTOR, "hello from app", 200_000);

        // user should have received the excess back; the app should hold zero.
        assertEq(address(app).balance, 0, "app retained ETH");
        assertEq(user.balance, overpay - fee, "user did not receive refund");
    }

    function testSendRejectsUnknownRemote() public {
        uint64 unknownSelector = 99_999;
        vm.expectRevert(abi.encodeWithSelector(ExampleCcipApp.UnknownRemoteApp.selector, unknownSelector));
        app.send(unknownSelector, "should fail", 200_000);
    }
}
