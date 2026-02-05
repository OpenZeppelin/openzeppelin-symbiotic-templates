// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {ISettlement} from "../src/interfaces/ISettlement.sol";
import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";
import {MessageV1Codec} from "../src/ccv/libraries/MessageV1Codec.sol";

contract SettlementStub is ISettlement {
    bool public signatureValid = true;
    uint48 public captureTimestamp;
    uint8 public keyTag = 15;
    uint256 public quorumThreshold = 6600;

    function setSignatureValid(bool value) external {
        signatureValid = value;
    }

    function setCaptureTimestamp(uint48 value) external {
        captureTimestamp = value;
    }

    function verifyQuorumSigAt(
        bytes memory,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external view override returns (bool) {
        return signatureValid;
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external view override returns (uint8) {
        return keyTag;
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external view override returns (uint256) {
        return quorumThreshold;
    }

    function getCaptureTimestampFromValSetHeaderAt(uint48) external view override returns (uint48) {
        return captureTimestamp;
    }
}

contract SymbioticCCVTest is Test {
    uint64 internal constant SOURCE_CHAIN = 31337;
    uint64 internal constant DEST_CHAIN = 31338;

    address internal onRamp = makeAddr("onRamp");
    address internal offRamp = makeAddr("offRamp");
    address internal sender = makeAddr("sender");

    SettlementStub internal settlement;
    SymbioticCCV internal ccv;

    function setUp() public {
        settlement = new SettlementStub();
        settlement.setCaptureTimestamp(uint48(block.timestamp));

        string[] memory locations = new string[](1);
        locations[0] = "mock://symbiotic-ccv/verifier-results";

        ccv = new SymbioticCCV(address(settlement), locations);

        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            onRamp: onRamp,
            offRamp: offRamp,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 250000,
            payloadSizeBytes: 128
        });
        ccv.applyRemoteChainConfigUpdates(args);

        // Also configure reverse lane for destination verification checks (source -> local).
        SymbioticCCV.RemoteChainConfigArgs[] memory reverseArgs = new SymbioticCCV.RemoteChainConfigArgs[](1);
        reverseArgs[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: SOURCE_CHAIN,
            onRamp: onRamp,
            offRamp: offRamp,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 250000,
            payloadSizeBytes: 128
        });
        ccv.applyRemoteChainConfigUpdates(reverseArgs);
    }

    function test_getOutboundImplementation_returnsSelf_whenSupported() public view {
        assertEq(ccv.getOutboundImplementation(DEST_CHAIN, ""), address(ccv));
    }

    function test_getOutboundImplementation_returnsZero_whenUnsupported() public view {
        assertEq(ccv.getOutboundImplementation(999999, ""), address(0));
    }

    function test_getInboundImplementation_returnsSelf_withValidVersion() public view {
        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"01");
        assertEq(ccv.getInboundImplementation(verifierResults), address(ccv));
    }

    function test_getInboundImplementation_returnsZero_withInvalidVersion() public view {
        bytes memory verifierResults = abi.encodePacked(bytes4(0x01020304), bytes6(uint48(1)), hex"01");
        assertEq(ccv.getInboundImplementation(verifierResults), address(0));
    }

    function test_forwardToVerifier_happyPath() public {
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(sender));

        vm.prank(onRamp);
        bytes memory out = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(bytes4(out), ccv.VERSION_TAG_V1_0_0());
    }

    function test_forwardToVerifier_revertsWhenWrongCaller() public {
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(sender));

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.CallerIsNotConfiguredOnRamp.selector, address(this)));
        ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");
    }

    function test_forwardToVerifier_revertsWhenAllowlistEnabledAndSenderMissing() public {
        SymbioticCCV.AllowlistConfigArgs[] memory updates = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            allowlistEnabled: true,
            addedAllowlistedSenders: new address[](0),
            removedAllowlistedSenders: new address[](0)
        });
        ccv.applyAllowlistUpdates(updates);

        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(sender));

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.SenderNotAllowed.selector, sender));
        ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");
    }

    function test_forwardToVerifier_allowlistedSenderPasses() public {
        address[] memory adds = new address[](1);
        adds[0] = sender;

        SymbioticCCV.AllowlistConfigArgs[] memory updates = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            allowlistEnabled: true,
            addedAllowlistedSenders: adds,
            removedAllowlistedSenders: new address[](0)
        });
        ccv.applyAllowlistUpdates(updates);

        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(sender));

        vm.prank(onRamp);
        bytes memory out = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(bytes4(out), ccv.VERSION_TAG_V1_0_0());
    }

    function test_verifyMessage_happyPath() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_verifyMessage_revertsWhenWrongCaller() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.CallerIsNotConfiguredOffRamp.selector, address(this)));
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_verifyMessage_revertsWhenInvalidVersion() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        bytes memory verifierResults = abi.encodePacked(bytes4(0x01020304), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidCCVVersion.selector, bytes4(0x01020304)));
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_verifyMessage_revertsWhenEpochInvalid() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        settlement.setCaptureTimestamp(0);

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        vm.expectRevert(SymbioticCCV.InvalidEpoch.selector);
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_verifyMessage_revertsWhenEpochStale() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        uint256 maxValidity = ccv.MAX_EPOCH_VALIDITY();
        vm.warp(maxValidity + 100);
        settlement.setCaptureTimestamp(uint48(block.timestamp - maxValidity - 1));

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        vm.expectRevert(SymbioticCCV.EpochTooStale.selector);
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_verifyMessage_revertsWhenQuorumSignatureInvalid() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        settlement.setSignatureValid(false);

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        vm.expectRevert(SymbioticCCV.InvalidQuorumSignature.selector);
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function _messageForForward(bytes memory encodedSender) internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.destChainSelector = DEST_CHAIN;
        message.sender = encodedSender;
    }

    function _messageForVerify() internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.sourceChainSelector = SOURCE_CHAIN;
    }
}
