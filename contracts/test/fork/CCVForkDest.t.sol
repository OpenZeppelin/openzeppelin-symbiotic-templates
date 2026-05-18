// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";

import {ISettlement} from "../../src/interfaces/ISettlement.sol";
import {SymbioticCCV} from "../../src/ccv/SymbioticCCV.sol";
import {ExampleCcipApp} from "../../src/examples/ExampleCcipApp.sol";
import {MessageV1Codec} from "../../src/ccv/libraries/MessageV1Codec.sol";

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

interface IOffRampExecute {
    function execute(
        bytes calldata encodedMessage,
        address[] calldata ccvs,
        bytes[] calldata verifierResults,
        uint32 gasLimitOverride
    ) external;
}

/// @notice Destination-side fork test against real Sepolia CCIP v2 staging deployment.
/// Constructs a MessageV1 as if emitted by the staging Base Sepolia OnRamp and
/// replays it through real Sepolia OffRamp.execute(...) with a mock-valid Settlement.
///
/// Validates that:
///   1. Sepolia OffRamp accepts messages from Base Sepolia OnRamp (sourceChain enabled + onRampHash allowlisted).
///   2. SymbioticCCV.verifyMessage is invoked correctly with our verifierResults.
///   3. ExampleCcipApp.ccipReceive is called via Router.routeMessage().
///
/// Run: forge test --fork-url $DEST_RPC_URL --match-contract CCVForkDest -vv
contract CCVForkDestTest is Test {
    address constant SEPOLIA_ROUTER = 0x784d49a71BB4C48eB7dA4cD7e6Ecb424f9b5EAB1;
    address constant SEPOLIA_OFFRAMP = 0x386577d8350D5814198974d16c3C756a638fBd62;
    address constant BASE_SEPOLIA_ONRAMP = 0x829F4e6E2B979a4B87Ecf493BE94e25087aa0Fcd;

    uint64 constant BASE_SEPOLIA_SELECTOR = 10_344_971_235_874_465_080;
    uint64 constant SEPOLIA_SELECTOR = 16_015_286_601_757_825_753;

    bytes4 constant VERSION_TAG_V1_0_0 = 0x1a75bd93;

    SymbioticCCV internal ccv;
    ExampleCcipApp internal app;
    address internal sourceApp;

    function setUp() public {
        require(block.chainid == 11_155_111, "expected Sepolia fork (chainid 11155111)");

        SettlementAlwaysValid settlement = new SettlementAlwaysValid();
        string[] memory locations = new string[](1);
        locations[0] = "mock://symbiotic-ccv/fork-dest";
        ccv = new SymbioticCCV(address(settlement), locations);

        SymbioticCCV.RemoteChainConfigArgs[] memory args = new SymbioticCCV.RemoteChainConfigArgs[](1);
        args[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: BASE_SEPOLIA_SELECTOR,
            onRamp: BASE_SEPOLIA_ONRAMP,
            offRamp: SEPOLIA_OFFRAMP,
            allowlistEnabled: false,
            feeUSDCents: 0,
            gasForVerification: 250_000,
            payloadSizeBytes: 1024
        });
        ccv.applyRemoteChainConfigUpdates(args);

        // executor address is only used source-side; any address works here.
        app = new ExampleCcipApp(SEPOLIA_ROUTER, address(ccv), makeAddr("unused-executor"));

        sourceApp = makeAddr("sourceApp");
        app.setRemoteApp(BASE_SEPOLIA_SELECTOR, sourceApp);
    }

    function testSepoliaOffRampHasCode() public view {
        assertGt(SEPOLIA_OFFRAMP.code.length, 0);
        assertGt(SEPOLIA_ROUTER.code.length, 0);
    }

    /// Drive a full destination-side execute. Builds a MessageV1 that mimics what
    /// the real Base Sepolia OnRamp would emit, encodes it, and submits via OffRamp.
    function testExecuteDeliversToCcipReceive() public {
        bytes memory encodedMessage = _buildEncodedMessage("hello dest fork");

        address[] memory ccvs = new address[](1);
        ccvs[0] = address(ccv);

        bytes[] memory verifierResults = new bytes[](1);
        // version(4) + epoch(6) + bls_sig (any non-empty bytes, Settlement is mocked).
        verifierResults[0] = abi.encodePacked(VERSION_TAG_V1_0_0, bytes6(uint48(1)), bytes("stub-signature"));

        vm.recordLogs();
        IOffRampExecute(SEPOLIA_OFFRAMP).execute(encodedMessage, ccvs, verifierResults, 0);

        Vm.Log[] memory logs = vm.getRecordedLogs();
        bytes32 messageReceivedTopic = keccak256("MessageReceived(uint64,bytes32,address,string)");
        bool gotReceive = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].emitter == address(app) && logs[i].topics[0] == messageReceivedTopic) {
                gotReceive = true;
                break;
            }
        }
        assertTrue(gotReceive, "ExampleCcipApp.MessageReceived not emitted");
    }

    // ─────────────────────────── helpers ───────────────────────────

    /// @dev Inline MessageV1 encoder matching the real CCIP v2 wire format.
    /// See chainlink-ccip:chains/evm/contracts/libraries/MessageV1Codec.sol:_encodeMessageV1.
    function _buildEncodedMessage(string memory message) internal view returns (bytes memory) {
        MessageV1Codec.MessageV1 memory m = MessageV1Codec.MessageV1({
            sourceChainSelector: BASE_SEPOLIA_SELECTOR,
            destChainSelector: SEPOLIA_SELECTOR,
            messageNumber: 1,
            executionGasLimit: 500_000,
            ccipReceiveGasLimit: 200_000,
            finality: bytes4(0),
            ccvAndExecutorHash: bytes32(0),
            onRampAddress: abi.encode(BASE_SEPOLIA_ONRAMP),
            offRampAddress: abi.encodePacked(SEPOLIA_OFFRAMP),
            sender: abi.encode(sourceApp),
            receiver: abi.encodePacked(address(app)),
            destBlob: new bytes(0),
            tokenTransfer: new MessageV1Codec.TokenTransferV1[](0),
            data: abi.encode(message)
        });

        return abi.encodePacked(
            abi.encodePacked(
                uint8(1),
                m.sourceChainSelector,
                m.destChainSelector,
                m.messageNumber,
                m.executionGasLimit,
                m.ccipReceiveGasLimit,
                m.finality,
                m.ccvAndExecutorHash
            ),
            abi.encodePacked(
                uint8(m.onRampAddress.length),
                m.onRampAddress,
                uint8(m.offRampAddress.length),
                m.offRampAddress,
                uint8(m.sender.length),
                m.sender
            ),
            abi.encodePacked(
                uint8(m.receiver.length),
                m.receiver,
                uint16(m.destBlob.length),
                m.destBlob,
                uint16(0), // encodedTokenTransfers length (no tokens)
                uint16(m.data.length),
                m.data
            )
        );
    }
}
