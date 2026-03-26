// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {ISettlement} from "../src/interfaces/ISettlement.sol";
import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";
import {ICrossChainVerifierResolver} from "../src/ccv/interfaces/ICrossChainVerifierResolver.sol";
import {ICrossChainVerifierV1} from "../src/ccv/interfaces/ICrossChainVerifierV1.sol";
import {Client} from "../src/ccv/libraries/Client.sol";
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

    function test_supportsInterface_resolver() public view {
        assertTrue(ccv.supportsInterface(type(ICrossChainVerifierResolver).interfaceId));
    }

    function test_forwardToVerifier_happyPath() public {
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(sender));

        vm.prank(onRamp);
        bytes memory out = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(bytes4(out), ccv.VERSION_TAG_V1_0_0());
    }

    function test_forwardToVerifier_accepts20ByteSenderEncoding() public {
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encodePacked(sender));

        vm.prank(onRamp);
        bytes memory out = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(bytes4(out), ccv.VERSION_TAG_V1_0_0());
    }

    function test_forwardToVerifier_revertsOnDirtyUpperBytes() public {
        bytes32 dirtySender = bytes32((uint256(1) << 248) | uint256(uint160(sender)));
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(dirtySender));

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidSenderEncodingUpperBytes.selector, dirtySender));
        ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");
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

    // ============ Input validation: applyRemoteChainConfigUpdates ============

    function test_applyRemoteChainConfigUpdates_revertsOnZeroRemoteChainSelector() public {
        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: 0,
            onRamp: onRamp,
            offRamp: offRamp,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 250000,
            payloadSizeBytes: 128
        });

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidRemoteChainConfig.selector, uint64(0)));
        ccv.applyRemoteChainConfigUpdates(args);
    }

    function test_applyRemoteChainConfigUpdates_revertsOnZeroOnRamp() public {
        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: 99,
            onRamp: address(0),
            offRamp: offRamp,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 250000,
            payloadSizeBytes: 128
        });

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidRemoteChainConfig.selector, uint64(99)));
        ccv.applyRemoteChainConfigUpdates(args);
    }

    function test_applyRemoteChainConfigUpdates_revertsOnZeroOffRamp() public {
        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: 99,
            onRamp: onRamp,
            offRamp: address(0),
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 250000,
            payloadSizeBytes: 128
        });

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidRemoteChainConfig.selector, uint64(99)));
        ccv.applyRemoteChainConfigUpdates(args);
    }

    function test_applyRemoteChainConfigUpdates_revertsOnZeroGasForVerification() public {
        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: 99,
            onRamp: onRamp,
            offRamp: offRamp,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 0,
            payloadSizeBytes: 128
        });

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidRemoteChainConfig.selector, uint64(99)));
        ccv.applyRemoteChainConfigUpdates(args);
    }

    // ============ Input validation: applyAllowlistUpdates ============

    function test_applyAllowlistUpdates_revertsForUnsupportedChain() public {
        SymbioticCCV.AllowlistConfigArgs[] memory updates = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: 999999,
            allowlistEnabled: true,
            addedAllowlistedSenders: new address[](0),
            removedAllowlistedSenders: new address[](0)
        });

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.RemoteChainNotSupported.selector, uint64(999999)));
        ccv.applyAllowlistUpdates(updates);
    }

    // ============ Constructor validation ============

    function test_constructor_revertsOnZeroSettlementAddress() public {
        string[] memory locations = new string[](0);

        vm.expectRevert(SymbioticCCV.ZeroAddressNotAllowed.selector);
        new SymbioticCCV(address(0), locations);
    }

    // ============ State/conditional ============

    function test_forwardToVerifier_allowlistDisabled_acceptsUnallowlistedSender() public {
        // Default config has allowlistEnabled=false, so any sender should pass
        address randomSender = makeAddr("randomSender");
        MessageV1Codec.MessageV1 memory message = _messageForForward(abi.encode(randomSender));

        vm.prank(onRamp);
        bytes memory out = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(bytes4(out), ccv.VERSION_TAG_V1_0_0());
    }

    function test_verifyMessage_epochAtExactMaxValidity() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        uint256 maxValidity = ccv.MAX_EPOCH_VALIDITY();
        // Set captureTimestamp such that block.timestamp == captureTimestamp + MAX_EPOCH_VALIDITY exactly
        uint48 captureTime = uint48(block.timestamp);
        settlement.setCaptureTimestamp(captureTime);
        vm.warp(uint256(captureTime) + maxValidity);

        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        // Should NOT revert - epoch is exactly at the boundary (not past it)
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_applyAllowlistUpdates_multipleAdditionsAndRemovals() public {
        address sender1 = makeAddr("sender1");
        address sender2 = makeAddr("sender2");

        // Add two senders
        address[] memory adds = new address[](2);
        adds[0] = sender1;
        adds[1] = sender2;

        SymbioticCCV.AllowlistConfigArgs[] memory updates = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            allowlistEnabled: true,
            addedAllowlistedSenders: adds,
            removedAllowlistedSenders: new address[](0)
        });
        ccv.applyAllowlistUpdates(updates);

        assertTrue(ccv.isSenderAllowlisted(DEST_CHAIN, sender1));
        assertTrue(ccv.isSenderAllowlisted(DEST_CHAIN, sender2));

        // Remove sender1
        address[] memory removes = new address[](1);
        removes[0] = sender1;

        SymbioticCCV.AllowlistConfigArgs[] memory updates2 = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates2[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            allowlistEnabled: true,
            addedAllowlistedSenders: new address[](0),
            removedAllowlistedSenders: removes
        });
        ccv.applyAllowlistUpdates(updates2);

        assertFalse(ccv.isSenderAllowlisted(DEST_CHAIN, sender1));
        assertTrue(ccv.isSenderAllowlisted(DEST_CHAIN, sender2));
    }

    function test_applyAllowlistUpdates_noEventWhenEmptyArrays() public {
        SymbioticCCV.AllowlistConfigArgs[] memory updates = new SymbioticCCV.AllowlistConfigArgs[](1);
        updates[0] = SymbioticCCV.AllowlistConfigArgs({
            remoteChainSelector: DEST_CHAIN,
            allowlistEnabled: false,
            addedAllowlistedSenders: new address[](0),
            removedAllowlistedSenders: new address[](0)
        });

        vm.recordLogs();
        ccv.applyAllowlistUpdates(updates);
        Vm.Log[] memory logs = vm.getRecordedLogs();

        // No AllowListSendersAdded or AllowListSendersRemoved events should be emitted
        bytes32 addedTopic = keccak256("AllowListSendersAdded(uint64,address[])");
        bytes32 removedTopic = keccak256("AllowListSendersRemoved(uint64,address[])");
        for (uint256 i = 0; i < logs.length; i++) {
            assertTrue(logs[i].topics[0] != addedTopic, "Should not emit AllowListSendersAdded");
            assertTrue(logs[i].topics[0] != removedTopic, "Should not emit AllowListSendersRemoved");
        }
    }

    // ============ Interface/query ============

    function test_supportsInterface_allValidInterfaces() public view {
        assertTrue(ccv.supportsInterface(type(ICrossChainVerifierV1).interfaceId));
        assertTrue(ccv.supportsInterface(type(ICrossChainVerifierResolver).interfaceId));
        assertTrue(ccv.supportsInterface(type(IERC165).interfaceId));
    }

    function test_supportsInterface_invalidInterface() public view {
        assertFalse(ccv.supportsInterface(bytes4(0xdeadbeef)));
        assertFalse(ccv.supportsInterface(bytes4(0x00000000)));
    }

    function test_getRemoteChainConfig_returnsCorrectConfig() public view {
        SymbioticCCV.RemoteChainConfig memory cfg = ccv.getRemoteChainConfig(DEST_CHAIN);
        assertEq(cfg.onRamp, onRamp);
        assertEq(cfg.offRamp, offRamp);
        assertEq(cfg.feeUSDCents, 42);
        assertEq(cfg.gasForVerification, 250000);
        assertEq(cfg.payloadSizeBytes, 128);
        assertFalse(cfg.allowlistEnabled);
    }

    function test_isSenderAllowlisted_returnsCorrectStatus() public {
        assertFalse(ccv.isSenderAllowlisted(DEST_CHAIN, sender));

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

        assertTrue(ccv.isSenderAllowlisted(DEST_CHAIN, sender));
    }

    function test_getStorageLocations_returnsCorrectLocations() public view {
        string[] memory locations = ccv.getStorageLocations();
        assertEq(locations.length, 1);
        assertEq(locations[0], "mock://symbiotic-ccv/verifier-results");
    }

    function test_updateStorageLocations_replacesExisting() public {
        string[] memory newLocations = new string[](2);
        newLocations[0] = "new://location-1";
        newLocations[1] = "new://location-2";

        ccv.updateStorageLocations(newLocations);

        string[] memory locations = ccv.getStorageLocations();
        assertEq(locations.length, 2);
        assertEq(locations[0], "new://location-1");
        assertEq(locations[1], "new://location-2");
    }

    // ============ Edge cases ============

    function test_getInboundImplementation_withShortPayload() public view {
        // 0 bytes
        assertEq(ccv.getInboundImplementation(""), address(0));
        // 1 byte
        assertEq(ccv.getInboundImplementation(hex"01"), address(0));
        // 2 bytes
        assertEq(ccv.getInboundImplementation(hex"0102"), address(0));
        // 3 bytes
        assertEq(ccv.getInboundImplementation(hex"010203"), address(0));
    }

    function test_verifyMessage_revertsOnTooShortVerifierResults() public {
        MessageV1Codec.MessageV1 memory message = _messageForVerify();
        bytes32 messageId = keccak256("msg-1");

        // MIN_VERIFIER_RESULTS_BYTES = 4 + 6 + 1 = 11; send 10 bytes
        bytes memory tooShort = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)));

        vm.prank(offRamp);
        vm.expectRevert(SymbioticCCV.InvalidVerifierResults.selector);
        ccv.verifyMessage(message, messageId, tooShort);
    }

    function test_verifyMessage_revertsOnUnsupportedChain() public {
        MessageV1Codec.MessageV1 memory message;
        message.sourceChainSelector = 999999;
        bytes32 messageId = keccak256("msg-1");
        bytes memory verifierResults = abi.encodePacked(ccv.VERSION_TAG_V1_0_0(), bytes6(uint48(1)), hex"abcd");

        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.RemoteChainNotSupported.selector, uint64(999999)));
        ccv.verifyMessage(message, messageId, verifierResults);
    }

    function test_getFee_revertsOnUnsupportedChain() public {
        Client.EVM2AnyMessage memory clientMsg;

        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.RemoteChainNotSupported.selector, uint64(999999)));
        ccv.getFee(999999, clientMsg, "", 0);
    }

    function test_getFee_returnsCorrectValues() public view {
        Client.EVM2AnyMessage memory clientMsg;

        (uint16 fee, uint32 gas, uint32 payload) = ccv.getFee(DEST_CHAIN, clientMsg, "", 0);
        assertEq(fee, 42);
        assertEq(gas, 250000);
        assertEq(payload, 128);
    }

    function test_forwardToVerifier_revertsOnUnsupportedChain() public {
        MessageV1Codec.MessageV1 memory message;
        message.destChainSelector = 999999;
        message.sender = abi.encode(sender);

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.RemoteChainNotSupported.selector, uint64(999999)));
        ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");
    }

    function test_forwardToVerifier_revertsOnInvalidSenderLength() public {
        // 15 bytes - not 20 or 32
        bytes memory badSender = new bytes(15);
        MessageV1Codec.MessageV1 memory message;
        message.destChainSelector = DEST_CHAIN;
        message.sender = badSender;

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticCCV.InvalidSenderEncoding.selector, uint256(15)));
        ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");
    }

    function _messageForForward(bytes memory encodedSender) internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.destChainSelector = DEST_CHAIN;
        message.sender = encodedSender;
    }

    function _messageForVerify() internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.sourceChainSelector = SOURCE_CHAIN;
    }
}
