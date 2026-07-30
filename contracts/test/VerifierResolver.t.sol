// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {CREATE2Factory} from "@chainlink/contracts-ccip/contracts/CREATE2Factory.sol";
import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";
import {VersionedVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/ccvs/VersionedVerifierResolver.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {SymbioticVerifier} from "../src/chainlink/SymbioticVerifier.sol";
import {ISettlement} from "../src/interfaces/ISettlement.sol";
import {MockCCIPOffRamp} from "../src/chainlink/mocks/MockCCIPOffRamp.sol";
import {MockCCIPOnRamp} from "../src/chainlink/mocks/MockCCIPOnRamp.sol";
import {MockRMN} from "../src/chainlink/mocks/MockRMN.sol";
import {MockRouter} from "../src/chainlink/mocks/MockRouter.sol";

contract ResolverSettlementStub is ISettlement {
    function verifyQuorumSigAt(
        bytes memory,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external pure override returns (bool) {
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

contract VerifierResolverTest is Test {
    bytes4 internal constant VERSION_V1 = 0x1a75bd93;
    bytes4 internal constant VERSION_V2 = 0x11223344;
    bytes32 internal constant RESOLVER_SALT = keccak256("resolver-test-salt");
    uint64 internal constant SOURCE_CHAIN = 31337;
    uint64 internal constant DEST_CHAIN_A = 31338;
    uint64 internal constant DEST_CHAIN_B = 31339;

    string internal constant RESOLVER_BYTECODE_PATH =
        "node_modules/@chainlink/contracts-ccip/bytecode/v2_0_0/versioned_verifier_resolver.bin";

    VersionedVerifierResolver internal resolver;

    function setUp() public {
        resolver = new VersionedVerifierResolver();
    }

    function test_prefixRouting_v1AndV2() public {
        address verifierV1 = makeAddr("verifierV1");
        address verifierV2 = makeAddr("verifierV2");
        _setInbound(resolver, VERSION_V1, verifierV1);
        _setInbound(resolver, VERSION_V2, verifierV2);

        assertEq(resolver.getInboundImplementation(abi.encodePacked(VERSION_V1, hex"0102")), verifierV1);
        assertEq(resolver.getInboundImplementation(abi.encodePacked(VERSION_V2, hex"0304")), verifierV2);
    }

    function test_unknownTag_returnsZeroAddress() public view {
        assertEq(resolver.getInboundImplementation(abi.encodePacked(bytes4(0xaabbccdd))), address(0));
    }

    function test_shortVerifierResults_reverts() public {
        vm.expectRevert(VersionedVerifierResolver.InvalidVerifierResultsLength.selector);
        resolver.getInboundImplementation(hex"010203");
    }

    function test_perLaneOutboundSwitch() public {
        address verifierV1 = makeAddr("verifierV1");
        address verifierV2 = makeAddr("verifierV2");
        _setOutbound(resolver, DEST_CHAIN_A, verifierV1);
        _setOutbound(resolver, DEST_CHAIN_B, verifierV1);
        _setOutbound(resolver, DEST_CHAIN_A, verifierV2);

        assertEq(resolver.getOutboundImplementation(DEST_CHAIN_A, ""), verifierV2);
        assertEq(resolver.getOutboundImplementation(DEST_CHAIN_B, ""), verifierV1);
    }

    function test_zeroAddressDeletesInboundAndOutbound() public {
        _setInbound(resolver, VERSION_V1, makeAddr("verifier"));
        _setOutbound(resolver, DEST_CHAIN_A, makeAddr("verifier"));
        _setInbound(resolver, VERSION_V1, address(0));
        _setOutbound(resolver, DEST_CHAIN_A, address(0));

        assertEq(resolver.getInboundImplementation(abi.encodePacked(VERSION_V1)), address(0));
        assertEq(resolver.getOutboundImplementation(DEST_CHAIN_A, ""), address(0));
        assertEq(resolver.getAllInboundImplementations().length, 0);
        assertEq(resolver.getAllOutboundImplementations().length, 0);
    }

    function test_coexistenceUpgrade_v1MessageVerifiesAfterV2BecomesOutbound() public {
        ResolverSettlementStub settlement = new ResolverSettlementStub();
        MockRMN rmn = new MockRMN();
        MockRouter router = new MockRouter();
        address offRamp = makeAddr("offRamp");
        router.setOffRamp(SOURCE_CHAIN, offRamp, true);

        SymbioticVerifier verifierV1 = _deployVerifier(settlement, rmn, router, VERSION_V1, SOURCE_CHAIN);
        SymbioticVerifier verifierV2 = _deployVerifier(settlement, rmn, router, VERSION_V2, SOURCE_CHAIN);
        _setInbound(resolver, VERSION_V1, address(verifierV1));
        _setOutbound(resolver, DEST_CHAIN_A, address(verifierV1));

        bytes memory inFlightV1Results = _verifierResults(VERSION_V1);

        _setInbound(resolver, VERSION_V2, address(verifierV2));
        _setOutbound(resolver, DEST_CHAIN_A, address(verifierV2));

        assertEq(resolver.getOutboundImplementation(DEST_CHAIN_A, ""), address(verifierV2));
        address inboundImplementation = resolver.getInboundImplementation(inFlightV1Results);
        assertEq(inboundImplementation, address(verifierV1));

        MessageV1Codec.MessageV1 memory message;
        message.sourceChainSelector = SOURCE_CHAIN;
        vm.prank(offRamp);
        SymbioticVerifier(inboundImplementation).verifyMessage(message, keccak256("in-flight-v1"), inFlightV1Results);
    }

    function test_factoryOwnershipHandoff_requiresAcceptOwnership() public {
        address resolverOwner = makeAddr("resolverOwner");
        CREATE2Factory factory = _factoryAllowing(address(this));
        bytes memory creationCode = _publishedResolverCreationCode();
        address deployed = factory.createAndTransferOwnership(creationCode, RESOLVER_SALT, resolverOwner);
        VersionedVerifierResolver deployedResolver = VersionedVerifierResolver(deployed);

        assertEq(deployedResolver.owner(), address(factory));
        vm.prank(resolverOwner);
        vm.expectRevert(bytes4(keccak256("OnlyCallableByOwner()")));
        deployedResolver.applyInboundImplementationUpdates(
            new VersionedVerifierResolver.InboundImplementationArgs[](0)
        );

        vm.prank(resolverOwner);
        deployedResolver.acceptOwnership();
        assertEq(deployedResolver.owner(), resolverOwner);

        vm.prank(resolverOwner);
        VersionedVerifierResolver.InboundImplementationArgs[] memory updates =
            new VersionedVerifierResolver.InboundImplementationArgs[](1);
        updates[0] = VersionedVerifierResolver.InboundImplementationArgs({
            version: VERSION_V1, verifier: makeAddr("verifier")
        });
        deployedResolver.applyInboundImplementationUpdates(updates);
    }

    function test_resolver_create2_publishedBytecodeMatchesComputedAddress() public {
        address resolverOwner = makeAddr("resolverOwner");
        CREATE2Factory factory = _factoryAllowing(address(this));
        bytes memory creationCode = _publishedResolverCreationCode();
        address predicted = factory.computeAddress(creationCode, RESOLVER_SALT);

        address deployed = factory.createAndTransferOwnership(creationCode, RESOLVER_SALT, resolverOwner);
        assertEq(deployed, predicted);
        assertGt(deployed.code.length, 0);

        vm.prank(resolverOwner);
        VersionedVerifierResolver(deployed).acceptOwnership();
        assertEq(VersionedVerifierResolver(deployed).owner(), resolverOwner);
    }

    function test_mockRampsResolveVerifierThroughResolver() public {
        ResolverSettlementStub settlement = new ResolverSettlementStub();
        MockRMN rmn = new MockRMN();
        MockRouter router = new MockRouter();
        SymbioticVerifier verifier = new SymbioticVerifier(
            address(settlement), _locations(), address(rmn), VERSION_V1
        );
        MockCCIPOnRamp onRamp = new MockCCIPOnRamp(address(resolver));
        MockCCIPOffRamp offRamp = new MockCCIPOffRamp(SOURCE_CHAIN);
        router.setOnRamp(DEST_CHAIN_A, address(onRamp));
        router.setOffRamp(SOURCE_CHAIN, address(offRamp), true);
        _configure(verifier, router, DEST_CHAIN_A);
        _configure(verifier, router, SOURCE_CHAIN);
        _setInbound(resolver, VERSION_V1, address(verifier));
        _setOutbound(resolver, DEST_CHAIN_A, address(verifier));

        MessageV1Codec.MessageV1 memory message;
        message.sourceChainSelector = SOURCE_CHAIN;
        message.destChainSelector = DEST_CHAIN_A;
        message.messageNumber = 1;
        message.onRampAddress = abi.encode(address(onRamp));
        message.offRampAddress = abi.encodePacked(address(offRamp));
        message.sender = abi.encode(address(this));
        message.receiver = abi.encodePacked(makeAddr("receiver"));
        message.tokenTransfer = new MessageV1Codec.TokenTransferV1[](0);
        bytes memory encodedMessage = MessageV1Codec._encodeMessageV1(message);

        bytes32 messageId = onRamp.sendMessage(DEST_CHAIN_A, encodedMessage, VERSION_V1, makeAddr("executor"));
        assertEq(messageId, keccak256(encodedMessage));

        address[] memory ccvs = new address[](1);
        ccvs[0] = address(resolver);
        bytes[] memory results = new bytes[](1);
        results[0] = _verifierResults(VERSION_V1);
        offRamp.execute(encodedMessage, ccvs, results, 0);
    }

    function _deployVerifier(
        ResolverSettlementStub settlement,
        MockRMN rmn,
        MockRouter router,
        bytes4 version,
        uint64 selector
    ) internal returns (SymbioticVerifier verifier) {
        verifier = new SymbioticVerifier(address(settlement), _locations(), address(rmn), version);
        _configure(verifier, router, selector);
    }

    function _configure(SymbioticVerifier verifier, MockRouter router, uint64 selector) internal {
        BaseVerifier.RemoteChainConfigArgs[] memory configs = new BaseVerifier.RemoteChainConfigArgs[](1);
        configs[0] = BaseVerifier.RemoteChainConfigArgs({
            router: IRouter(address(router)),
            remoteChainSelector: selector,
            allowlistEnabled: false,
            feeUSDCents: 0,
            gasForVerification: 250_000,
            payloadSizeBytes: 128
        });
        verifier.applyRemoteChainConfigUpdates(configs);
    }

    function _setInbound(VersionedVerifierResolver target, bytes4 version, address verifier) internal {
        VersionedVerifierResolver.InboundImplementationArgs[] memory updates =
            new VersionedVerifierResolver.InboundImplementationArgs[](1);
        updates[0] = VersionedVerifierResolver.InboundImplementationArgs({
            version: version, verifier: verifier
        });
        target.applyInboundImplementationUpdates(updates);
    }

    function _setOutbound(VersionedVerifierResolver target, uint64 selector, address verifier) internal {
        VersionedVerifierResolver.OutboundImplementationArgs[] memory updates =
            new VersionedVerifierResolver.OutboundImplementationArgs[](1);
        updates[0] = VersionedVerifierResolver.OutboundImplementationArgs({
            destChainSelector: selector, verifier: verifier
        });
        target.applyOutboundImplementationUpdates(updates);
    }

    function _factoryAllowing(address caller) internal returns (CREATE2Factory factory) {
        address[] memory allowList = new address[](1);
        allowList[0] = caller;
        factory = new CREATE2Factory(allowList);
    }

    function _publishedResolverCreationCode() internal view returns (bytes memory) {
        return vm.parseBytes(vm.trim(vm.readFile(RESOLVER_BYTECODE_PATH)));
    }

    function _locations() internal pure returns (string[] memory locations) {
        locations = new string[](1);
        locations[0] = "https://operator.example/verifications";
    }

    function _verifierResults(bytes4 version) internal pure returns (bytes memory) {
        return abi.encodePacked(version, bytes6(uint48(1)), hex"abcd");
    }
}
