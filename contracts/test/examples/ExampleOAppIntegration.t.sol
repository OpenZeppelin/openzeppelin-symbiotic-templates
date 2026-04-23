// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { Test, Vm } from "forge-std/Test.sol";

// OZ5-compatible mock contracts from test-devtools
import {
    EndpointV2Mock as EndpointV2
} from "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/EndpointV2Mock.sol";
import {
    ReceiveUln302Mock as ReceiveUln302
} from "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/ReceiveUln302Mock.sol";

// Config structs from messagelib-v2
import { SetDefaultUlnConfigParam, UlnConfig } from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {
    SetDefaultExecutorConfigParam,
    ExecutorConfig
} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";
import { Origin } from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";

// Our contracts
import { SymbioticLayerZeroDVN } from "../../src/SymbioticLayerZeroDVN.sol";
import { ILayerZeroDVN } from "../../src/interfaces/ILayerZeroDVN.sol";
import { MockSettlement } from "../../src/mocks/MockSettlement.sol";
import { ExampleOApp } from "../../src/examples/ExampleOApp.sol";
import { OptionsBuilder } from "@layerzerolabs/oapp-evm/contracts/oapp/libs/OptionsBuilder.sol";

/// @title MockSendUln302
/// @notice Minimal SendUln302 mock that calls DVN.assignJob and emits necessary events
/// @dev This is a simplified version that doesn't use TestHelperOz5's packet scheduling
contract MockSendUln302 {
    address public immutable endpoint;
    address payable public dvn;
    address public executor;

    uint32 constant CONFIG_TYPE_EXECUTOR = 1;
    uint32 constant CONFIG_TYPE_ULN = 2;

    // Store last packet for testing
    bytes public lastPacketHeader;
    bytes32 public lastPayloadHash;

    event PacketSent(bytes encodedPacket, bytes options, uint256 fee);

    constructor(address _endpoint) {
        endpoint = _endpoint;
    }

    function setDvn(address payable _dvn) external {
        dvn = _dvn;
    }

    function setExecutor(address _executor) external {
        executor = _executor;
    }

    /// @notice Simplified send that just assigns job to DVN
    function send(
        uint32 dstEid,
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations,
        address sender,
        bytes calldata options
    )
        external
        payable
        returns (uint256 fee)
    {
        // Store for testing
        lastPacketHeader = packetHeader;
        lastPayloadHash = payloadHash;

        // Call DVN.assignJob
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: dstEid,
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: confirmations,
            sender: sender
        });

        fee = SymbioticLayerZeroDVN(dvn).assignJob{ value: msg.value }(param, options);

        emit PacketSent(packetHeader, options, fee);
        return fee;
    }

    /// @notice Quote fee (delegates to DVN)
    function quote(
        uint32 dstEid,
        uint64 confirmations,
        address sender,
        bytes calldata options
    )
        external
        view
        returns (uint256)
    {
        return SymbioticLayerZeroDVN(dvn).getFee(dstEid, confirmations, sender, options);
    }
}

/// @title ExampleOAppIntegration
/// @notice Integration test for ExampleOApp with real LayerZero protocol and Symbiotic DVN
/// @dev Tests the full message flow from source to destination using SymbioticLayerZeroDVN
contract ExampleOAppIntegrationTest is Test {
    using OptionsBuilder for bytes;

    // Chain configurations
    uint32 constant SOURCE_EID = 31_337;
    uint32 constant DEST_EID = 31_338;
    uint64 constant CONFIRMATIONS = 1;

    // Source chain contracts
    EndpointV2 public srcEndpoint;
    SymbioticLayerZeroDVN public srcDvn;
    ExampleOApp public srcOApp;

    // Destination chain contracts
    EndpointV2 public dstEndpoint;
    ReceiveUln302 public dstReceiveUln;
    MockSettlement public settlement;
    SymbioticLayerZeroDVN public dstDvn;
    ExampleOApp public dstOApp;

    // Test helpers
    MockSendUln302 public srcSendUln;

    address public owner;
    address public user;
    address public submitter;

    function setUp() public {
        owner = address(this);
        user = makeAddr("user");
        submitter = makeAddr("submitter");

        vm.deal(user, 100 ether);
        vm.deal(submitter, 10 ether);

        // ============ Deploy Source Chain Infrastructure ============

        // 1. Deploy source endpoint
        srcEndpoint = new EndpointV2(SOURCE_EID, owner);

        // 2. Deploy mock SendUln302 (simplified for testing)
        srcSendUln = new MockSendUln302(address(srcEndpoint));

        // 3. Deploy source DVN (only needs sendUln)
        srcDvn = new SymbioticLayerZeroDVN(
            address(0), // settlement not needed on source
            address(srcSendUln), // sendUln
            address(0), // receiveUln not needed on source
            SOURCE_EID,
            0 // baseFee
        );
        srcSendUln.setDvn(payable(address(srcDvn)));

        // 4. Deploy source OApp
        srcOApp = new ExampleOApp(address(srcEndpoint), owner);

        // ============ Deploy Destination Chain Infrastructure ============

        // 1. Deploy destination endpoint
        dstEndpoint = new EndpointV2(DEST_EID, owner);

        // 2. Deploy ReceiveUln302Mock
        dstReceiveUln = new ReceiveUln302(address(dstEndpoint));

        // 3. Register ReceiveUln302 with endpoint
        dstEndpoint.registerLibrary(address(dstReceiveUln));

        // 4. Deploy MockSettlement (always returns true for signatures)
        settlement = new MockSettlement();

        // 5. Deploy destination DVN (needs settlement + receiveUln)
        dstDvn = new SymbioticLayerZeroDVN(
            address(settlement),
            address(0), // sendUln not needed on dest
            address(dstReceiveUln),
            DEST_EID,
            0 // baseFee
        );

        // 6. Add submitter to DVN whitelist
        dstDvn.addSubmitter(submitter);

        // 7. Configure ReceiveUln302 with our DVN
        _configureReceiveUln();

        // 8. Set default receive library
        dstEndpoint.setDefaultReceiveLibrary(SOURCE_EID, address(dstReceiveUln), 0);

        // 9. Deploy destination OApp
        dstOApp = new ExampleOApp(address(dstEndpoint), owner);

        // ============ Configure Peers ============
        bytes32 srcOAppBytes32 = bytes32(uint256(uint160(address(srcOApp))));
        bytes32 dstOAppBytes32 = bytes32(uint256(uint160(address(dstOApp))));

        srcOApp.setPeer(DEST_EID, dstOAppBytes32);
        dstOApp.setPeer(SOURCE_EID, srcOAppBytes32);
    }

    function _configureReceiveUln() internal {
        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = address(dstDvn);
        address[] memory optionalDVNs = new address[](0);

        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: SOURCE_EID,
            config: UlnConfig({
                confirmations: CONFIRMATIONS,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: requiredDVNs,
                optionalDVNs: optionalDVNs
            })
        });
        dstReceiveUln.setDefaultUlnConfigs(ulnParams);
    }

    /// @notice Build a LayerZero packet header (81 bytes)
    /// @dev Format: version (1) + nonce (8) + srcEid (4) + sender (32) + dstEid (4) + receiver (32)
    function _buildPacketHeader(
        uint8 version,
        uint64 nonce,
        uint32 srcEid,
        address sender,
        uint32 dstEid,
        address receiver
    )
        internal
        pure
        returns (bytes memory)
    {
        return abi.encodePacked(
            version, nonce, srcEid, bytes32(uint256(uint160(sender))), dstEid, bytes32(uint256(uint160(receiver)))
        );
    }

    /// @notice Test the full DVN verification flow from source to destination
    /// @dev Tests: assignJob -> submitProof -> verify -> commitVerification
    /// Note: lzReceive delivery is endpoint-specific and tested separately
    function test_fullMessageFlow() public {
        // Step 1: Build packet and payload
        bytes memory packetHeader = _buildPacketHeader(1, 1, SOURCE_EID, address(srcOApp), DEST_EID, address(dstOApp));
        bytes memory payload = abi.encode("Hello from source chain!");
        bytes32 payloadHash = keccak256(payload);

        // Step 2: Send message (triggers DVN.assignJob)
        _sendMessage(packetHeader, payloadHash);

        // Step 3: Submit proof on destination chain (DVN verification)
        _submitProof(packetHeader, payloadHash);

        // Step 4: Commit verification (ULN to Endpoint)
        dstReceiveUln.commitVerification(packetHeader, payloadHash);

        // Step 5: Verify the full DVN + ULN flow completed
        bytes32 leaf = dstDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        assertTrue(dstDvn.isLeafVerified(leaf), "DVN: Leaf should be verified");
        assertTrue(dstDvn.isRootVerified(leaf), "DVN: Root should be cached");
        // Note: lzReceive delivery to OApp depends on endpoint mock behavior
        // The critical DVN verification path is tested above
    }

    function _sendMessage(bytes memory packetHeader, bytes32 payloadHash) internal {
        bytes memory options = srcOApp.buildOptions(200_000);

        // Record logs to capture JobAssigned event
        vm.recordLogs();

        // Call send on mock SendUln which triggers DVN.assignJob
        vm.prank(user);
        srcSendUln.send(DEST_EID, packetHeader, payloadHash, CONFIRMATIONS, address(srcOApp), options);

        // Verify JobAssigned event was emitted
        Vm.Log[] memory logs = vm.getRecordedLogs();
        bool foundJobAssigned = false;
        for (uint256 i = 0; i < logs.length; i++) {
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
        assertTrue(foundJobAssigned, "JobAssigned event not emitted");
    }

    function _submitProof(bytes memory packetHeader, bytes32 payloadHash) internal {
        // For single message, leaf = root
        bytes32 leaf = dstDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32[] memory merkleProof = new bytes32[](0);

        // Build signature with epoch (MockSettlement always returns true)
        bytes memory signature = abi.encodePacked(uint48(block.timestamp), bytes("fake_bls_signature"));

        vm.prank(submitter);
        dstDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, merkleProof, leaf, signature);

        assertTrue(dstDvn.isLeafVerified(leaf), "Leaf not marked as verified");
        assertTrue(dstDvn.isRootVerified(leaf), "Root not marked as verified");
    }

    /// @notice Test that submitting proof twice for same leaf fails
    function test_revertDuplicateProof() public {
        bytes memory packetHeader = _buildPacketHeader(1, 1, SOURCE_EID, address(srcOApp), DEST_EID, address(dstOApp));
        bytes32 payloadHash = keccak256(abi.encode("test message"));

        bytes32 leaf = dstDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = leaf;
        bytes32[] memory merkleProof = new bytes32[](0);

        uint48 epoch = uint48(block.timestamp);
        bytes memory signature = abi.encodePacked(epoch, bytes("fake_sig"));

        // First submission should succeed
        vm.prank(submitter);
        dstDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, merkleProof, merkleRoot, signature);

        // Second submission should fail
        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.AlreadyVerified.selector);
        dstDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, merkleProof, merkleRoot, signature);
    }

    /// @notice Test that cached root can be reused without signature
    function test_cachedRootReuse() public {
        // First message to cache the root
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, address(srcOApp), DEST_EID, address(dstOApp));
        bytes32 payloadHash1 = keccak256(abi.encode("message 1"));

        // Second message using same root (batched)
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, address(srcOApp), DEST_EID, address(dstOApp));
        bytes32 payloadHash2 = keccak256(abi.encode("message 2"));

        // Build Merkle tree with two leaves
        bytes32 leaf1 = dstDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = dstDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        // Simple 2-leaf tree: root = keccak256(leaf1, leaf2) if leaf1 < leaf2
        bytes32 merkleRoot;
        bytes32[] memory proof1 = new bytes32[](1);
        bytes32[] memory proof2 = new bytes32[](1);

        if (uint256(leaf1) < uint256(leaf2)) {
            merkleRoot = keccak256(abi.encodePacked(leaf1, leaf2));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        } else {
            merkleRoot = keccak256(abi.encodePacked(leaf2, leaf1));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        }

        uint48 epoch = uint48(block.timestamp);
        bytes memory signature = abi.encodePacked(epoch, bytes("fake_sig"));

        // First submission with signature
        vm.prank(submitter);
        dstDvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof1, merkleRoot, signature);

        assertTrue(dstDvn.isRootVerified(merkleRoot), "Root should be cached");

        // Second submission without signature (root already cached)
        vm.prank(submitter);
        dstDvn.submitProof(packetHeader2, payloadHash2, CONFIRMATIONS, proof2, merkleRoot, "");

        assertTrue(dstDvn.isLeafVerified(leaf1), "Leaf1 should be verified");
        assertTrue(dstDvn.isLeafVerified(leaf2), "Leaf2 should be verified");
    }

    /// @notice Test that only authorized submitters can submit proofs
    function test_revertUnauthorizedSubmitter() public {
        bytes memory packetHeader = _buildPacketHeader(1, 1, SOURCE_EID, address(srcOApp), DEST_EID, address(dstOApp));
        bytes32 payloadHash = keccak256(abi.encode("test"));

        bytes32 leaf = dstDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        uint48 epoch = uint48(block.timestamp);
        bytes memory signature = abi.encodePacked(epoch, bytes("sig"));

        // Random address tries to submit - should fail
        address randomAddr = makeAddr("random");
        vm.prank(randomAddr);
        vm.expectRevert(abi.encodeWithSelector(SymbioticLayerZeroDVN.UnauthorizedSubmitter.selector, randomAddr));
        dstDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    /// @notice Test that wrong destination chain reverts
    function test_revertWrongDestinationChain() public {
        // Build packet header with wrong destination
        bytes memory packetHeader = _buildPacketHeader(1, 1, SOURCE_EID, address(srcOApp), SOURCE_EID, address(dstOApp)); // Wrong: SOURCE_EID instead of DEST_EID

        bytes32 payloadHash = keccak256(abi.encode("test"));
        bytes32 leaf = dstDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);

        uint48 epoch = uint48(block.timestamp);
        bytes memory signature = abi.encodePacked(epoch, bytes("sig"));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.WrongDestinationChain.selector);
        dstDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    /// @notice Test DVN fee mechanism
    function test_dvnFee() public {
        // Update base fee
        uint256 newFee = 0.001 ether;
        srcDvn.setBaseFee(newFee);

        // Check fee is returned correctly
        uint256 fee = srcDvn.getFee(DEST_EID, CONFIRMATIONS, address(srcOApp), "");
        assertEq(fee, newFee, "Fee incorrect");
    }

    /// @notice Test submitter management
    function test_submitterManagement() public {
        address newSubmitter = makeAddr("newSubmitter");

        // Initially not a submitter
        assertFalse(dstDvn.isSubmitter(newSubmitter));

        // Add submitter
        dstDvn.addSubmitter(newSubmitter);
        assertTrue(dstDvn.isSubmitter(newSubmitter));

        // Cannot add twice
        vm.expectRevert(SymbioticLayerZeroDVN.SubmitterAlreadyAuthorized.selector);
        dstDvn.addSubmitter(newSubmitter);

        // Remove submitter
        dstDvn.removeSubmitter(newSubmitter);
        assertFalse(dstDvn.isSubmitter(newSubmitter));

        // Cannot remove twice
        vm.expectRevert(SymbioticLayerZeroDVN.SubmitterNotAuthorized.selector);
        dstDvn.removeSubmitter(newSubmitter);
    }
}
