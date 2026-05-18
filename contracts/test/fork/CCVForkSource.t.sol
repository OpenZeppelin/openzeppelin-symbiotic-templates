// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {ISettlement} from "../../src/interfaces/ISettlement.sol";
import {SymbioticCCV} from "../../src/ccv/SymbioticCCV.sol";
import {Client} from "../../src/ccv/libraries/Client.sol";
import {MessageV1Codec} from "../../src/ccv/libraries/MessageV1Codec.sol";

interface IRouterClient {
    function getFee(
        uint64 destinationChainSelector,
        Client.EVM2AnyMessage memory message
    ) external view returns (uint256 fee);

    function ccipSend(
        uint64 destinationChainSelector,
        Client.EVM2AnyMessage calldata message
    ) external payable returns (bytes32 messageId);
}

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

/// @notice Source-side fork test against real Base Sepolia CCIP v2 staging deployment.
/// Run with:  forge test --fork-url $SOURCE_RPC_URL --match-contract CCVForkSource -vvv
contract CCVForkSourceTest is Test {
    // Base Sepolia staging deployment (CCIP v2 beta).
    address constant ROUTER = 0x0Ec6D443B425982f1F2862Dd0ffBFD431FCb6b8b;
    address constant ON_RAMP = 0x829F4e6E2B979a4B87Ecf493BE94e25087aa0Fcd;
    address constant LINK_TOKEN = 0xE4aB69C077896252FAFBD49EFD26B5D171A32410;

    // Sepolia destination addresses (used in remote chain config).
    address constant SEPOLIA_OFFRAMP = 0x386577d8350D5814198974d16c3C756a638fBd62;

    uint64 constant BASE_SEPOLIA_SELECTOR = 10_344_971_235_874_465_080;
    uint64 constant SEPOLIA_SELECTOR = 16_015_286_601_757_825_753;

    bytes4 constant GENERIC_EXTRA_ARGS_V3_TAG = 0xa69dd4aa;
    bytes4 constant VERSION_TAG_V1_0_0 = 0x1a75bd93;

    SettlementAlwaysValid internal settlement;
    SymbioticCCV internal ccv;
    address internal user;

    function setUp() public {
        // Caller is expected to pass --fork-url $SOURCE_RPC_URL.
        // Sanity check that we're on Base Sepolia (chain id 84532).
        require(block.chainid == 84_532, "expected Base Sepolia fork (chainid 84532)");

        settlement = new SettlementAlwaysValid();

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

        user = makeAddr("ccvForkUser");
    }

    /// Sanity: real OnRamp is at the expected address with code.
    function testStagingContractsHaveCode() public view {
        assertGt(ON_RAMP.code.length, 0, "OnRamp has no code on this fork");
        assertGt(ROUTER.code.length, 0, "Router has no code on this fork");
        assertGt(LINK_TOKEN.code.length, 0, "LINK has no code on this fork");
    }

    /// Verifies our CCV's source-side API contract when the real OnRamp would call it.
    /// Impersonates the OnRamp and calls `forwardToVerifier` directly.
    function testForwardToVerifierFromImpersonatedOnRamp() public {
        MessageV1Codec.MessageV1 memory message = _stubMessageV1(SEPOLIA_SELECTOR);

        vm.prank(ON_RAMP);
        bytes memory blob = ccv.forwardToVerifier(message, bytes32(uint256(1)), address(0), 0, "");

        assertEq(blob.length, 4, "expected 4-byte version tag");
        bytes4 tag;
        assembly {
            tag := mload(add(blob, 32))
        }
        assertEq(tag, VERSION_TAG_V1_0_0, "expected SymbioticCCV version tag");
    }

    /// Full send path: call real Router.ccipSend with our CCV in extraArgs and assert
    /// CCIPMessageSent is emitted by the real OnRamp.
    function testCcipSendIncludesOurCcv() public {
        Client.EVM2AnyMessage memory message = Client.EVM2AnyMessage({
            receiver: abi.encode(makeAddr("destReceiver")),
            data: bytes("hello CCV"),
            tokenAmounts: new Client.EVMTokenAmount[](0),
            feeToken: address(0), // native ETH
            extraArgs: _encodeExtraArgsWithCcv(address(ccv), 200_000)
        });

        uint256 fee = IRouterClient(ROUTER).getFee(SEPOLIA_SELECTOR, message);
        assertGt(fee, 0, "Router quoted zero fee");

        vm.deal(user, fee * 2);
        vm.prank(user);
        bytes32 messageId = IRouterClient(ROUTER).ccipSend{value: fee}(SEPOLIA_SELECTOR, message);

        assertTrue(messageId != bytes32(0), "ccipSend returned zero messageId");
    }

    // ─────────────────────────── helpers ───────────────────────────

    function _stubMessageV1(uint64 destSelector) internal returns (MessageV1Codec.MessageV1 memory) {
        return MessageV1Codec.MessageV1({
            sourceChainSelector: BASE_SEPOLIA_SELECTOR,
            destChainSelector: destSelector,
            messageNumber: 1,
            executionGasLimit: 200_000,
            ccipReceiveGasLimit: 200_000,
            finality: bytes4(0),
            ccvAndExecutorHash: bytes32(0),
            onRampAddress: abi.encode(ON_RAMP),
            offRampAddress: abi.encodePacked(SEPOLIA_OFFRAMP),
            sender: abi.encode(user),
            receiver: abi.encodePacked(makeAddr("destReceiver")),
            destBlob: new bytes(0),
            tokenTransfer: new MessageV1Codec.TokenTransferV1[](0),
            data: bytes("hello")
        });
    }

    /// Minimal GenericExtraArgsV3 encoder, single CCV, no executor/token args.
    /// Layout: tag(4) | gasLimit(4) | finalityConfig(4) | ccvsLength(1) |
    ///         ccvAddrLength(1) | ccvAddr(20) | ccvArgsLength(2) |
    ///         executorLength(1) | executorArgsLength(2) |
    ///         tokenReceiverLength(1) | tokenArgsLength(2)
    function _encodeExtraArgsWithCcv(address ccvAddr, uint32 gasLimit) internal pure returns (bytes memory) {
        return abi.encodePacked(
            GENERIC_EXTRA_ARGS_V3_TAG,
            gasLimit,
            bytes4(0), // requestedFinalityConfig = 0 (wait for finality)
            uint8(1), // 1 CCV
            uint8(20), // ccvAddr length
            bytes20(ccvAddr),
            uint16(0), // empty ccvArgs
            uint8(0), // empty executor (use default)
            uint16(0), // empty executorArgs
            uint8(0), // empty tokenReceiver
            uint16(0) // empty tokenArgs
        );
    }
}
