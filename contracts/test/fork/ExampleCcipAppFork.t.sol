// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {ISettlement} from "../../src/interfaces/ISettlement.sol";
import {SymbioticCCV} from "../../src/ccv/SymbioticCCV.sol";
import {ExampleCcipApp} from "../../src/examples/ExampleCcipApp.sol";
import {NoOpExecutor} from "../../src/examples/NoOpExecutor.sol";

contract SettlementAlwaysValid is ISettlement {
    function verifyQuorumSigAt(bytes memory, uint8, uint256, bytes calldata, uint48, bytes memory)
        external
        pure
        override
        returns (bool)
    {
        return true;
    }
    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure override returns (uint8) {
        return 15;
    }
    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure override returns (uint256) {
        return 6600;
    }
    function getCaptureTimestampFromValSetHeaderAt(uint48) external view override returns (uint48) {
        return uint48(block.timestamp);
    }
}

/// @notice Fork test that drives the full source-side flow through ExampleCcipApp
/// rather than calling Router.ccipSend directly. Validates that the unified
/// app contract correctly assembles extraArgs and pays the Router.
contract ExampleCcipAppForkTest is Test {
    address constant ROUTER = 0x0Ec6D443B425982f1F2862Dd0ffBFD431FCb6b8b;
    address constant ON_RAMP = 0x829F4e6E2B979a4B87Ecf493BE94e25087aa0Fcd;
    address constant SEPOLIA_OFFRAMP = 0x386577d8350D5814198974d16c3C756a638fBd62;

    uint64 constant SEPOLIA_SELECTOR = 16_015_286_601_757_825_753;

    SymbioticCCV internal ccv;
    ExampleCcipApp internal app;
    address internal operator;
    address internal user;

    function setUp() public {
        require(block.chainid == 84_532, "expected Base Sepolia fork (chainid 84532)");

        SettlementAlwaysValid settlement = new SettlementAlwaysValid();
        string[] memory locations = new string[](1);
        locations[0] = "mock://symbiotic-ccv/fork-source";
        ccv = new SymbioticCCV(address(settlement), locations);

        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: SEPOLIA_SELECTOR,
            onRamp: ON_RAMP,
            offRamp: SEPOLIA_OFFRAMP,
            allowlistEnabled: false,
            feeUSDCents: 0,
            gasForVerification: 250_000,
            payloadSizeBytes: 1024
        });
        ccv.applyRemoteChainConfigUpdates(args);

        operator = address(new NoOpExecutor());
        app = new ExampleCcipApp(ROUTER, address(ccv), operator);

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
