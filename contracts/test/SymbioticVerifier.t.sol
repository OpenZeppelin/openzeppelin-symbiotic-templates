// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";

import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {ICrossChainVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierResolver.sol";
import {ICrossChainVerifierV1} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierV1.sol";
import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {Client} from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";
import {FinalityCodec} from "@chainlink/contracts-ccip/contracts/libraries/FinalityCodec.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {SymbioticVerifier} from "../src/chainlink/SymbioticVerifier.sol";
import {ISettlement} from "../src/interfaces/ISettlement.sol";
import {MockRMN} from "../src/chainlink/mocks/MockRMN.sol";
import {MockRouter} from "../src/chainlink/mocks/MockRouter.sol";

contract SettlementStub is ISettlement {
    bool public signatureValid = true;
    bool public checkExpectedDigest;
    uint48 public captureTimestamp;
    uint8 public keyTag = 15;
    uint256 public quorumThreshold = 6600;
    bytes32 public expectedDigest;

    function setSignatureValid(bool value) external {
        signatureValid = value;
    }

    function setCaptureTimestamp(uint48 value) external {
        captureTimestamp = value;
    }

    function setExpectedDigest(bytes32 value) external {
        expectedDigest = value;
        checkExpectedDigest = true;
    }

    function verifyQuorumSigAt(
        bytes memory data,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external view override returns (bool) {
        return signatureValid && (!checkExpectedDigest || abi.decode(data, (bytes32)) == expectedDigest);
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

contract SymbioticVerifierTest is Test {
    uint64 internal constant SOURCE_CHAIN = 31337;
    uint64 internal constant DEST_CHAIN = 31338;
    bytes4 internal constant VERSION_TAG = 0x1a75bd93;

    address internal onRamp = makeAddr("onRamp");
    address internal offRamp = makeAddr("offRamp");
    address internal sender = makeAddr("sender");

    SettlementStub internal settlement;
    MockRouter internal router;
    MockRMN internal rmn;
    SymbioticVerifier internal verifier;

    function setUp() public {
        settlement = new SettlementStub();
        settlement.setCaptureTimestamp(uint48(block.timestamp));
        router = new MockRouter();
        rmn = new MockRMN();

        string[] memory locations = new string[](1);
        locations[0] = "https://operator.example/verifications";
        verifier = new SymbioticVerifier(address(settlement), locations, address(rmn), VERSION_TAG);

        router.setOnRamp(DEST_CHAIN, onRamp);
        router.setOffRamp(SOURCE_CHAIN, offRamp, true);
        _configure(DEST_CHAIN, address(router), false, 42, 250_000, 128);
        _configure(SOURCE_CHAIN, address(router), false, 42, 250_000, 128);
    }

    function test_versionTag_preservesOperatorVersion() public view {
        assertEq(verifier.versionTag(), VERSION_TAG);
    }

    function test_typeAndVersion() public view {
        assertEq(verifier.typeAndVersion(), "SymbioticVerifier 1.0.0");
    }

    function test_supportsInterface_verifierAndERC165() public view {
        assertTrue(verifier.supportsInterface(type(ICrossChainVerifierV1).interfaceId));
        assertTrue(verifier.supportsInterface(type(IERC165).interfaceId));
    }

    function test_supportsInterface_doesNotClaimResolver() public view {
        assertFalse(verifier.supportsInterface(type(ICrossChainVerifierResolver).interfaceId));
    }

    function test_forwardToVerifier_happyPath() public {
        vm.prank(onRamp);
        bytes memory result = verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
        assertEq(bytes4(result), VERSION_TAG);
    }

    function test_forwardToVerifier_accepts20ByteSenderEncoding() public {
        vm.prank(onRamp);
        bytes memory result = verifier.forwardToVerifier(
            _messageForForward(abi.encodePacked(sender)), bytes32(0), address(0), 0, ""
        );
        assertEq(bytes4(result), VERSION_TAG);
    }

    function test_forwardToVerifier_revertsOnDirtyUpperBytes() public {
        bytes32 dirtySender = bytes32((uint256(1) << 248) | uint256(uint160(sender)));
        vm.prank(onRamp);
        vm.expectRevert(
            abi.encodeWithSelector(SymbioticVerifier.InvalidSenderEncodingUpperBytes.selector, dirtySender)
        );
        verifier.forwardToVerifier(_messageForForward(abi.encode(dirtySender)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_revertsOnInvalidSenderLength() public {
        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticVerifier.InvalidSenderEncoding.selector, uint256(15)));
        verifier.forwardToVerifier(_messageForForward(new bytes(15)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_revertsWhenWrongCaller() public {
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CallerIsNotARampOnRouter.selector, address(this)));
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_resolvesRotatedOnRamp() public {
        address newOnRamp = makeAddr("newOnRamp");
        router.setOnRamp(DEST_CHAIN, newOnRamp);

        vm.prank(newOnRamp);
        bytes memory result =
            verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
        assertEq(bytes4(result), VERSION_TAG);

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CallerIsNotARampOnRouter.selector, onRamp));
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_zeroRouterPausesLane() public {
        _configure(DEST_CHAIN, address(0), false, 42, 250_000, 128);

        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.RemoteChainNotSupported.selector, DEST_CHAIN));
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_revertsWhenAllowlistEnabledAndSenderMissing() public {
        _setAllowlist(DEST_CHAIN, true, new address[](0), new address[](0));
        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.SenderNotAllowed.selector, sender));
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
    }

    function test_forwardToVerifier_allowlistedSenderPasses() public {
        address[] memory additions = new address[](1);
        additions[0] = sender;
        _setAllowlist(DEST_CHAIN, true, additions, new address[](0));

        vm.prank(onRamp);
        bytes memory result = verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
        assertEq(bytes4(result), VERSION_TAG);
    }

    function test_forwardToVerifier_allowlistDisabledAcceptsUnlistedSender() public {
        vm.prank(onRamp);
        bytes memory result = verifier.forwardToVerifier(
            _messageForForward(abi.encode(makeAddr("unlisted"))), bytes32(0), address(0), 0, ""
        );
        assertEq(bytes4(result), VERSION_TAG);
    }

    function test_verifyMessage_happyPathAndDigest() public {
        bytes32 messageId = keccak256("message");
        settlement.setExpectedDigest(keccak256(bytes.concat(VERSION_TAG, messageId)));

        vm.prank(offRamp);
        verifier.verifyMessage(_messageForVerify(), messageId, _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsWhenWrongCaller() public {
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CallerIsNotARampOnRouter.selector, address(this)));
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_resolvesRotatedOffRamp() public {
        address newOffRamp = makeAddr("newOffRamp");
        router.setOffRamp(SOURCE_CHAIN, offRamp, false);
        router.setOffRamp(SOURCE_CHAIN, newOffRamp, true);

        vm.prank(newOffRamp);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));

        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CallerIsNotARampOnRouter.selector, offRamp));
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_zeroRouterPausesLane() public {
        _configure(SOURCE_CHAIN, address(0), false, 42, 250_000, 128);

        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.RemoteChainNotSupported.selector, SOURCE_CHAIN));
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsWhenInvalidVersion() public {
        bytes4 invalidVersion = 0x01020304;
        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(SymbioticVerifier.InvalidCCVVersion.selector, invalidVersion));
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(invalidVersion, 1));
    }

    function test_verifyMessage_revertsWhenEpochInvalid() public {
        settlement.setCaptureTimestamp(0);
        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.InvalidEpoch.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsWhenEpochStale() public {
        vm.warp(verifier.getEpochValidity() + 100);
        settlement.setCaptureTimestamp(uint48(block.timestamp - verifier.getEpochValidity() - 1));
        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.EpochTooStale.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_epochAtExactMaxValidity() public {
        uint48 captureTime = uint48(block.timestamp);
        settlement.setCaptureTimestamp(captureTime);
        vm.warp(uint256(captureTime) + verifier.getEpochValidity());

        vm.prank(offRamp);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsWhenQuorumSignatureInvalid() public {
        settlement.setSignatureValid(false);
        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.InvalidQuorumSignature.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsOnTooShortVerifierResults() public {
        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.InvalidVerifierResults.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), abi.encodePacked(VERSION_TAG, bytes6(uint48(1))));
    }

    function test_verifyMessage_revertsOnUnsupportedChain() public {
        MessageV1Codec.MessageV1 memory message;
        message.sourceChainSelector = 999999;
        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.RemoteChainNotSupported.selector, uint64(999999)));
        verifier.verifyMessage(message, bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_verifyMessage_revertsWhenSourceCursed() public {
        rmn.setCursed(bytes16(uint128(SOURCE_CHAIN)), true);
        vm.prank(offRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CursedByRMN.selector, SOURCE_CHAIN));
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));

        rmn.setCursed(bytes16(uint128(SOURCE_CHAIN)), false);
        vm.prank(offRamp);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_forwardToVerifier_revertsWhenDestinationCursed() public {
        rmn.setCursed(bytes16(uint128(DEST_CHAIN)), true);
        vm.prank(onRamp);
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.CursedByRMN.selector, DEST_CHAIN));
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");

        rmn.setCursed(bytes16(uint128(DEST_CHAIN)), false);
        vm.prank(onRamp);
        verifier.forwardToVerifier(_messageForForward(abi.encode(sender)), bytes32(0), address(0), 0, "");
    }

    function test_applyRemoteChainConfigUpdates_revertsOnZeroRemoteChainSelector() public {
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.InvalidRemoteChainConfig.selector, uint64(0)));
        _configure(0, address(router), false, 42, 250_000, 128);
    }

    function test_applyRemoteChainConfigUpdates_revertsOnZeroGasForVerification() public {
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.DestGasCannotBeZero.selector, uint64(99)));
        _configure(99, address(router), false, 42, 0, 128);
    }

    function test_applyRemoteChainConfigUpdates_zeroRouterPausesLane() public {
        _configure(DEST_CHAIN, address(0), false, 42, 250_000, 128);
        Client.EVM2AnyMessage memory message;
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.RemoteChainNotSupported.selector, DEST_CHAIN));
        verifier.getFee(DEST_CHAIN, message, "", FinalityCodec.WAIT_FOR_FINALITY_FLAG);
    }

    function test_constructor_revertsOnZeroSettlementAddress() public {
        vm.expectRevert(BaseVerifier.ZeroAddressNotAllowed.selector);
        new SymbioticVerifier(address(0), new string[](0), address(rmn), VERSION_TAG);
    }

    function test_constructor_revertsOnZeroRmnAddress() public {
        vm.expectRevert(BaseVerifier.ZeroAddressNotAllowed.selector);
        new SymbioticVerifier(address(settlement), new string[](0), address(0), VERSION_TAG);
    }

    function test_constructor_revertsOnZeroVersionTag() public {
        vm.expectRevert(BaseVerifier.VersionTagCannotBeZero.selector);
        new SymbioticVerifier(address(settlement), new string[](0), address(rmn), bytes4(0));
    }

    function test_applyAllowlistUpdates_multipleAdditionsAndRemovals() public {
        address sender1 = makeAddr("sender1");
        address sender2 = makeAddr("sender2");
        address[] memory additions = new address[](2);
        additions[0] = sender1;
        additions[1] = sender2;
        _setAllowlist(DEST_CHAIN, true, additions, new address[](0));

        (, address[] memory allowedBefore) = verifier.getRemoteChainConfig(DEST_CHAIN);
        assertEq(allowedBefore.length, 2);

        address[] memory removals = new address[](1);
        removals[0] = sender1;
        _setAllowlist(DEST_CHAIN, true, new address[](0), removals);
        (, address[] memory allowedAfter) = verifier.getRemoteChainConfig(DEST_CHAIN);
        assertEq(allowedAfter.length, 1);
        assertEq(allowedAfter[0], sender2);
    }

    function test_applyAllowlistUpdates_emitsPerSenderAndStateEvents() public {
        address[] memory additions = new address[](1);
        additions[0] = sender;
        vm.recordLogs();
        _setAllowlist(DEST_CHAIN, true, additions, new address[](0));
        Vm.Log[] memory logs = vm.getRecordedLogs();

        assertEq(logs.length, 2);
        assertEq(logs[0].topics[0], keccak256("AllowListStateChanged(uint64,bool)"));
        assertEq(logs[1].topics[0], keccak256("AllowListSendersAdded(uint64,address)"));
    }

    function test_applyAllowlistUpdates_revertsWhenAddingWhileDisabled() public {
        address[] memory additions = new address[](1);
        additions[0] = sender;
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.InvalidAllowListRequest.selector, DEST_CHAIN));
        _setAllowlist(DEST_CHAIN, false, additions, new address[](0));
    }

    function test_getRemoteChainConfig_returnsTuple() public view {
        (BaseVerifier.RemoteChainConfigArgs memory config, address[] memory allowedSenders) =
            verifier.getRemoteChainConfig(DEST_CHAIN);
        assertEq(address(config.router), address(router));
        assertEq(config.remoteChainSelector, DEST_CHAIN);
        assertEq(config.feeUSDCents, 42);
        assertEq(config.gasForVerification, 250_000);
        assertEq(config.payloadSizeBytes, 128);
        assertFalse(config.allowlistEnabled);
        assertEq(allowedSenders.length, 0);
    }

    function test_getStorageLocations_returnsConfiguredLocations() public view {
        string[] memory locations = verifier.getStorageLocations();
        assertEq(locations.length, 1);
        assertEq(locations[0], "https://operator.example/verifications");
    }

    function test_updateStorageLocations_replacesExisting() public {
        string[] memory oldLocations = new string[](1);
        oldLocations[0] = "https://operator.example/verifications";
        string[] memory newLocations = new string[](2);
        newLocations[0] = "https://one.example/verifications";
        newLocations[1] = "https://two.example/verifications";

        vm.expectEmit(address(verifier));
        emit BaseVerifier.StorageLocationsUpdated(oldLocations, newLocations);
        verifier.updateStorageLocations(newLocations);

        string[] memory actual = verifier.getStorageLocations();
        assertEq(actual.length, 2);
        assertEq(actual[0], newLocations[0]);
        assertEq(actual[1], newLocations[1]);
    }

    function test_getFee_revertsOnUnsupportedChain() public {
        Client.EVM2AnyMessage memory message;
        vm.expectRevert(abi.encodeWithSelector(BaseVerifier.RemoteChainNotSupported.selector, uint64(999999)));
        verifier.getFee(999999, message, "", FinalityCodec.WAIT_FOR_FINALITY_FLAG);
    }

    function test_getFee_returnsCorrectValues() public view {
        Client.EVM2AnyMessage memory message;
        (uint16 fee, uint32 gasForVerification, uint32 payloadSize) =
            verifier.getFee(DEST_CHAIN, message, "", FinalityCodec.WAIT_FOR_FINALITY_FLAG);
        assertEq(fee, 42);
        assertEq(gasForVerification, 250_000);
        assertEq(payloadSize, 128);
    }

    function test_getFee_defaultFinalityRejectsBlockDepth() public {
        Client.EVM2AnyMessage memory message;
        bytes4 requestedFinality = FinalityCodec._encodeBlockDepth(1);
        vm.expectRevert(
            abi.encodeWithSelector(
                FinalityCodec.InvalidRequestedFinality.selector,
                requestedFinality,
                FinalityCodec.WAIT_FOR_FINALITY_FLAG
            )
        );
        verifier.getFee(DEST_CHAIN, message, "", requestedFinality);
    }

    function test_getFee_defaultFinalityRejectsWaitForSafe() public {
        Client.EVM2AnyMessage memory message;
        bytes4 requestedFinality = FinalityCodec.WAIT_FOR_SAFE_FLAG;
        vm.expectRevert(
            abi.encodeWithSelector(
                FinalityCodec.InvalidRequestedFinality.selector,
                requestedFinality,
                FinalityCodec.WAIT_FOR_FINALITY_FLAG
            )
        );
        verifier.getFee(DEST_CHAIN, message, "", requestedFinality);
    }

    function test_getFee_ownerCanAllowBlockDepthAndWaitForSafe() public {
        Client.EVM2AnyMessage memory message;
        verifier.setAllowedFinalityConfig(FinalityCodec._encodeBlockDepthAndSafeFlag(1));

        verifier.getFee(DEST_CHAIN, message, "", FinalityCodec._encodeBlockDepth(1));
        verifier.getFee(DEST_CHAIN, message, "", FinalityCodec.WAIT_FOR_SAFE_FLAG);
    }

    function test_transferOwnership_requiresAcceptanceBeforeControlChanges() public {
        address pendingOwner = makeAddr("pendingOwner");
        verifier.transferOwnership(pendingOwner);

        assertEq(verifier.owner(), address(this));
        verifier.setAllowedFinalityConfig(FinalityCodec.WAIT_FOR_SAFE_FLAG);

        vm.prank(pendingOwner);
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        verifier.setAllowedFinalityConfig(FinalityCodec.WAIT_FOR_FINALITY_FLAG);

        vm.prank(pendingOwner);
        verifier.acceptOwnership();
        assertEq(verifier.owner(), pendingOwner);

        vm.prank(pendingOwner);
        verifier.setAllowedFinalityConfig(FinalityCodec.WAIT_FOR_FINALITY_FLAG);
    }

    function test_transferOwnership_allowsOwnerToRecoverFromWrongPendingOwner() public {
        address wrongOwner = makeAddr("wrongOwner");
        address intendedOwner = makeAddr("intendedOwner");
        verifier.transferOwnership(wrongOwner);
        verifier.transferOwnership(intendedOwner);

        vm.prank(wrongOwner);
        vm.expectRevert(bytes4(keccak256("MustBeProposedOwner()")));
        verifier.acceptOwnership();

        vm.prank(intendedOwner);
        verifier.acceptOwnership();
        assertEq(verifier.owner(), intendedOwner);
    }

    function test_applyRemoteChainConfigUpdates_onlyOwner() public {
        vm.prank(makeAddr("notOwner"));
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        _configure(99, address(router), false, 42, 250_000, 128);
    }

    function test_applyAllowlistUpdates_onlyOwner() public {
        vm.prank(makeAddr("notOwner"));
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        _setAllowlist(DEST_CHAIN, true, new address[](0), new address[](0));
    }

    function test_updateStorageLocations_onlyOwner() public {
        vm.prank(makeAddr("notOwner"));
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        verifier.updateStorageLocations(new string[](0));
    }

    function test_setAllowedFinalityConfig_onlyOwner() public {
        vm.prank(makeAddr("notOwner"));
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        verifier.setAllowedFinalityConfig(bytes4(uint32(1)));
    }

    function test_setEpochValidity_defaultsToTwoHours() public view {
        assertEq(verifier.getEpochValidity(), verifier.DEFAULT_EPOCH_VALIDITY());
        assertEq(verifier.DEFAULT_EPOCH_VALIDITY(), 2 hours);
    }

    function test_setEpochValidity_updatesWindowAndEmits() public {
        vm.expectEmit(address(verifier));
        emit SymbioticVerifier.EpochValiditySet(24 hours);
        verifier.setEpochValidity(24 hours);
        assertEq(verifier.getEpochValidity(), 24 hours);
    }

    function test_setEpochValidity_raiseAndRestoreRecoversStaleEpoch() public {
        // Epoch captured 10h ago: stale under the 2h default.
        vm.warp(block.timestamp + 30 days);
        uint48 captureTime = uint48(block.timestamp - 10 hours);
        settlement.setCaptureTimestamp(captureTime);

        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.EpochTooStale.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));

        // Owner raises the window during incident recovery: same epoch verifies.
        verifier.setEpochValidity(24 hours);
        vm.prank(offRamp);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));

        // Restored to the default: the epoch is stale again.
        verifier.setEpochValidity(verifier.DEFAULT_EPOCH_VALIDITY());
        vm.prank(offRamp);
        vm.expectRevert(SymbioticVerifier.EpochTooStale.selector);
        verifier.verifyMessage(_messageForVerify(), bytes32(0), _verifierResults(VERSION_TAG, 1));
    }

    function test_setEpochValidity_revertsOutOfBounds() public {
        uint256 belowFloor = verifier.MIN_EPOCH_VALIDITY() - 1;
        vm.expectRevert(abi.encodeWithSelector(SymbioticVerifier.InvalidEpochValidity.selector, belowFloor));
        verifier.setEpochValidity(belowFloor);

        uint256 aboveCeiling = verifier.MAX_EPOCH_VALIDITY() + 1;
        vm.expectRevert(abi.encodeWithSelector(SymbioticVerifier.InvalidEpochValidity.selector, aboveCeiling));
        verifier.setEpochValidity(aboveCeiling);
    }

    function test_setEpochValidity_onlyOwner() public {
        vm.prank(makeAddr("notOwner"));
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        verifier.setEpochValidity(24 hours);
    }

    function _configure(
        uint64 selector,
        address routerAddress,
        bool allowlistEnabled,
        uint16 feeUSDCents,
        uint32 gasForVerification,
        uint16 payloadSizeBytes
    ) internal {
        BaseVerifier.RemoteChainConfigArgs[] memory updates = new BaseVerifier.RemoteChainConfigArgs[](1);
        updates[0] = BaseVerifier.RemoteChainConfigArgs({
            router: IRouter(routerAddress),
            remoteChainSelector: selector,
            allowlistEnabled: allowlistEnabled,
            feeUSDCents: feeUSDCents,
            gasForVerification: gasForVerification,
            payloadSizeBytes: payloadSizeBytes
        });
        verifier.applyRemoteChainConfigUpdates(updates);
    }

    function _setAllowlist(
        uint64 selector,
        bool enabled,
        address[] memory additions,
        address[] memory removals
    ) internal {
        BaseVerifier.AllowlistConfigArgs[] memory updates = new BaseVerifier.AllowlistConfigArgs[](1);
        updates[0] = BaseVerifier.AllowlistConfigArgs({
            destChainSelector: selector,
            allowlistEnabled: enabled,
            addedAllowlistedSenders: additions,
            removedAllowlistedSenders: removals
        });
        verifier.applyAllowlistUpdates(updates);
    }

    function _messageForForward(
        bytes memory encodedSender
    ) internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.destChainSelector = DEST_CHAIN;
        message.sender = encodedSender;
    }

    function _messageForVerify() internal pure returns (MessageV1Codec.MessageV1 memory message) {
        message.sourceChainSelector = SOURCE_CHAIN;
    }

    function _verifierResults(bytes4 version, uint48 epoch) internal pure returns (bytes memory) {
        return abi.encodePacked(version, bytes6(epoch), hex"abcd");
    }
}
