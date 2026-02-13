// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";

import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";
import {ILayerZeroDVN} from "../src/interfaces/ILayerZeroDVN.sol";
import {IReceiveUlnE2} from "../src/interfaces/IReceiveUlnE2.sol";
import {ISettlement} from "../src/interfaces/ISettlement.sol";
import {AssertingSettlement} from "./helpers/AssertingSettlement.sol";
import {ReentrantReceiveUln} from "./helpers/ReentrantReceiveUln.sol";
import {RevertingReceiveUln} from "./helpers/RevertingReceiveUln.sol";

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

contract ReceiveUlnStub is IReceiveUlnE2 {
    bytes public lastPacketHeader;
    bytes32 public lastPayloadHash;
    uint64 public lastConfirmations;
    uint256 public verifyCalls;

    function verify(bytes calldata _packetHeader, bytes32 _payloadHash, uint64 _confirmations) external override {
        lastPacketHeader = _packetHeader;
        lastPayloadHash = _payloadHash;
        lastConfirmations = _confirmations;
        verifyCalls += 1;
    }

    function commitVerification(bytes calldata, bytes32) external override {}
}

contract SymbioticLayerZeroDVNTest is Test {
    uint32 internal constant SOURCE_EID = 31337;
    uint32 internal constant DEST_EID = 31338;
    uint64 internal constant CONFIRMATIONS = 1;
    uint256 internal constant BASE_FEE = 0.01 ether;

    address internal constant SENDER = address(0xBEEF);
    address internal constant RECEIVER = address(0xCAFE);

    SymbioticLayerZeroDVN public sourceDvn;
    SymbioticLayerZeroDVN public destinationDvn;

    SettlementStub public settlement;
    ReceiveUlnStub public receiveUln;

    address public sendUln;
    address public submitter;
    address public other;

    function setUp() public {
        sendUln = makeAddr("sendUln");
        submitter = makeAddr("submitter");
        other = makeAddr("other");

        sourceDvn = new SymbioticLayerZeroDVN(address(0), sendUln, address(0), SOURCE_EID, BASE_FEE);

        settlement = new SettlementStub();
        receiveUln = new ReceiveUlnStub();
        settlement.setCaptureTimestamp(uint48(block.timestamp));

        destinationDvn = new SymbioticLayerZeroDVN(address(settlement), address(0), address(receiveUln), DEST_EID, 0);
        destinationDvn.addSubmitter(submitter);
    }

    function test_assignJob_returnsBaseFee() public {
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: _defaultPacketHeader(),
            payloadHash: keccak256(abi.encodePacked("payload")),
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        vm.prank(sendUln);
        uint256 fee = sourceDvn.assignJob(param, "");

        assertEq(fee, BASE_FEE);
    }

    function test_assignJob_revertsWhenNotSendUln() public {
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: _defaultPacketHeader(),
            payloadHash: keccak256(abi.encodePacked("payload")),
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        vm.expectRevert(SymbioticLayerZeroDVN.OnlySendUln.selector);
        sourceDvn.assignJob(param, "");
    }

    function test_assignJob_revertsWhenEthSent() public {
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: _defaultPacketHeader(),
            payloadHash: keccak256(abi.encodePacked("payload")),
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        vm.deal(sendUln, 1 ether);
        vm.prank(sendUln);
        vm.expectRevert(SymbioticLayerZeroDVN.NoFeeAccepted.selector);
        sourceDvn.assignJob{value: 1 ether}(param, "");
    }

    function test_assignJob_revertsWhenPacketHeaderInvalid() public {
        bytes memory shortHeader = new bytes(80);

        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: shortHeader,
            payloadHash: keccak256(abi.encodePacked("payload")),
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        vm.prank(sendUln);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidPacketHeader.selector);
        sourceDvn.assignJob(param, "");
    }

    function test_submitProof_happyPathCachesRootAndCallsReceiveUln() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32[] memory proof = new bytes32[](0);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, proof, leaf, signature);

        assertTrue(destinationDvn.isLeafVerified(leaf));
        assertTrue(destinationDvn.isRootVerified(leaf));
        assertEq(receiveUln.verifyCalls(), 1);
        assertEq(receiveUln.lastPayloadHash(), payloadHash);
        assertEq(receiveUln.lastConfirmations(), CONFIRMATIONS);
        assertEq(keccak256(receiveUln.lastPacketHeader()), keccak256(packetHeader));
    }

    function test_cacheMerkleRoot_happyPathCachesRoot() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.cacheMerkleRoot(merkleRoot, signature);

        assertTrue(destinationDvn.isRootVerified(merkleRoot));
        assertEq(receiveUln.verifyCalls(), 0);
    }

    function test_cacheMerkleRoot_preCachedRoot_allowsSubmitProofWithoutSignature() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 merkleRoot = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.cacheMerkleRoot(merkleRoot, signature);

        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, "");

        assertTrue(destinationDvn.isLeafVerified(merkleRoot));
        assertTrue(destinationDvn.isRootVerified(merkleRoot));
        assertEq(receiveUln.verifyCalls(), 1);
    }

    function test_cacheMerkleRoot_cachedRoot_isNoOp() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.cacheMerkleRoot(merkleRoot, signature);

        settlement.setCaptureTimestamp(0);
        settlement.setSignatureValid(false);

        vm.prank(submitter);
        destinationDvn.cacheMerkleRoot(merkleRoot, "");

        assertTrue(destinationDvn.isRootVerified(merkleRoot));
    }

    function test_cacheMerkleRoot_revertsWhenSignatureMissing() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.SignatureRequired.selector);
        destinationDvn.cacheMerkleRoot(merkleRoot, "");
    }

    function test_cacheMerkleRoot_revertsWhenSignatureTooShort() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));
        bytes memory shortSignature = new bytes(6);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.SignatureTooShort.selector);
        destinationDvn.cacheMerkleRoot(merkleRoot, shortSignature);
    }

    function test_cacheMerkleRoot_revertsWhenInvalidSignature() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));
        bytes memory signature = _buildSignature(uint48(block.timestamp));
        settlement.setSignatureValid(false);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidQuorumSignature.selector);
        destinationDvn.cacheMerkleRoot(merkleRoot, signature);
    }

    function test_cacheMerkleRoot_emitsMerkleRootCached_withCorrectEpochAndRoot() public {
        bytes32 merkleRoot = keccak256(abi.encodePacked("root"));
        uint48 epoch = 0x010203040506;
        bytes memory signature = abi.encodePacked(epoch, bytes("sig"));

        vm.expectEmit(true, true, true, true);
        emit SymbioticLayerZeroDVN.MerkleRootCached(merkleRoot, epoch);

        vm.prank(submitter);
        destinationDvn.cacheMerkleRoot(merkleRoot, signature);
    }

    function test_submitProofBatch_happyPathVerifiesAllLeaves() public {
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));

        bytes32 leaf1 = destinationDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = destinationDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        bytes32 merkleRoot;
        bytes32[] memory proof1 = new bytes32[](1);
        bytes32[] memory proof2 = new bytes32[](1);
        if (leaf1 < leaf2) {
            merkleRoot = keccak256(abi.encodePacked(leaf1, leaf2));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        } else {
            merkleRoot = keccak256(abi.encodePacked(leaf2, leaf1));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        }

        SymbioticLayerZeroDVN.BatchProof[] memory proofs = new SymbioticLayerZeroDVN.BatchProof[](2);
        proofs[0] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader1,
            payloadHash: payloadHash1,
            confirmations: CONFIRMATIONS,
            merkleProof: proof1
        });
        proofs[1] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader2,
            payloadHash: payloadHash2,
            confirmations: CONFIRMATIONS,
            merkleProof: proof2
        });

        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.submitProofBatch(proofs, merkleRoot, signature);

        assertTrue(destinationDvn.isLeafVerified(leaf1));
        assertTrue(destinationDvn.isLeafVerified(leaf2));
        assertTrue(destinationDvn.isRootVerified(merkleRoot));
        assertEq(receiveUln.verifyCalls(), 2);
    }

    function test_submitProofBatch_cachedRoot_allowsEmptySignature() public {
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));

        bytes32 leaf1 = destinationDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = destinationDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        bytes32 merkleRoot;
        bytes32[] memory proof1 = new bytes32[](1);
        bytes32[] memory proof2 = new bytes32[](1);
        if (leaf1 < leaf2) {
            merkleRoot = keccak256(abi.encodePacked(leaf1, leaf2));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        } else {
            merkleRoot = keccak256(abi.encodePacked(leaf2, leaf1));
            proof1[0] = leaf2;
            proof2[0] = leaf1;
        }

        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof1, merkleRoot, signature);

        SymbioticLayerZeroDVN.BatchProof[] memory proofs = new SymbioticLayerZeroDVN.BatchProof[](1);
        proofs[0] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader2,
            payloadHash: payloadHash2,
            confirmations: CONFIRMATIONS,
            merkleProof: proof2
        });

        vm.prank(submitter);
        destinationDvn.submitProofBatch(proofs, merkleRoot, "");

        assertTrue(destinationDvn.isLeafVerified(leaf1));
        assertTrue(destinationDvn.isLeafVerified(leaf2));
        assertEq(receiveUln.verifyCalls(), 2);
    }

    function test_submitProofBatch_revertsWhenEmptyBatch() public {
        SymbioticLayerZeroDVN.BatchProof[] memory proofs = new SymbioticLayerZeroDVN.BatchProof[](0);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.EmptyBatch.selector);
        destinationDvn.submitProofBatch(proofs, keccak256(abi.encodePacked("root")), "");
    }

    function test_submitProofBatch_revertsWhenBatchTooLarge() public {
        SymbioticLayerZeroDVN.BatchProof[] memory proofs =
            new SymbioticLayerZeroDVN.BatchProof[](destinationDvn.MAX_BATCH_SIZE() + 1);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.ProofTooLarge.selector);
        destinationDvn.submitProofBatch(proofs, keccak256(abi.encodePacked("root")), "");
    }

    function test_submitProofBatch_revertsWhenAnyProofInvalid_rollsBackState() public {
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));

        bytes32 leaf1 = destinationDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = destinationDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        bytes32 merkleRoot;
        bytes32[] memory proof1 = new bytes32[](1);
        bytes32[] memory proof2 = new bytes32[](1);
        if (leaf1 < leaf2) {
            merkleRoot = keccak256(abi.encodePacked(leaf1, leaf2));
            proof1[0] = leaf2;
            proof2[0] = bytes32(uint256(123456)); // invalid sibling
        } else {
            merkleRoot = keccak256(abi.encodePacked(leaf2, leaf1));
            proof1[0] = leaf2;
            proof2[0] = bytes32(uint256(123456)); // invalid sibling
        }

        SymbioticLayerZeroDVN.BatchProof[] memory proofs = new SymbioticLayerZeroDVN.BatchProof[](2);
        proofs[0] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader1,
            payloadHash: payloadHash1,
            confirmations: CONFIRMATIONS,
            merkleProof: proof1
        });
        proofs[1] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader2,
            payloadHash: payloadHash2,
            confirmations: CONFIRMATIONS,
            merkleProof: proof2
        });

        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidMerkleProof.selector);
        destinationDvn.submitProofBatch(proofs, merkleRoot, signature);

        assertFalse(destinationDvn.isLeafVerified(leaf1));
        assertFalse(destinationDvn.isLeafVerified(leaf2));
        assertFalse(destinationDvn.isRootVerified(merkleRoot));
        assertEq(receiveUln.verifyCalls(), 0);
    }

    function test_submitProofBatch_revertsWhenDuplicateLeafInBatch_rollsBackState() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32[] memory proof = new bytes32[](0);

        SymbioticLayerZeroDVN.BatchProof[] memory proofs = new SymbioticLayerZeroDVN.BatchProof[](2);
        proofs[0] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: CONFIRMATIONS,
            merkleProof: proof
        });
        proofs[1] = SymbioticLayerZeroDVN.BatchProof({
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: CONFIRMATIONS,
            merkleProof: proof
        });

        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.AlreadyVerified.selector);
        destinationDvn.submitProofBatch(proofs, leaf, signature);

        assertFalse(destinationDvn.isLeafVerified(leaf));
        assertFalse(destinationDvn.isRootVerified(leaf));
        assertEq(receiveUln.verifyCalls(), 0);
    }
    function test_submitProof_revertsWhenSignatureMissing() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.SignatureRequired.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, "");
    }

    function test_submitProof_revertsWhenSignatureTooLarge() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = new bytes(destinationDvn.MAX_SIGNATURE_SIZE() + 1);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.ProofTooLarge.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenMerkleProofTooLarge() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));
        bytes32[] memory oversizedProof = new bytes32[](destinationDvn.MAX_MERKLE_DEPTH() + 1);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.ProofTooLarge.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, oversizedProof, leaf, signature);
    }

    function test_submitProof_revertsWhenInvalidSignature() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        settlement.setSignatureValid(false);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidQuorumSignature.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenInvalidEpoch() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        settlement.setCaptureTimestamp(0);

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidEpoch.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenEpochTooStale() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        uint256 maxValidity = destinationDvn.MAX_EPOCH_VALIDITY();
        vm.warp(maxValidity + 100);
        settlement.setCaptureTimestamp(uint48(block.timestamp - maxValidity - 1));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.EpochTooStale.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenInvalidMerkleProof() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = keccak256(abi.encodePacked(leaf, bytes32(uint256(1))));
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidMerkleProof.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, signature);
    }

    function test_submitProof_revertsWhenInvalidPacketHeaderLength() public {
        bytes memory packetHeader = new bytes(80);
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidPacketHeader.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenWrongDestinationChain() public {
        bytes memory packetHeader =
            _buildPacketHeader(1, 1, SOURCE_EID, SENDER, SOURCE_EID, RECEIVER);
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.WrongDestinationChain.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenReceiveUlnNotSet() public {
        AssertingSettlement assertingSettlement = new AssertingSettlement();
        assertingSettlement.setShouldRevertOnAnyCall(true);
        SymbioticLayerZeroDVN noReceiveDvn =
            new SymbioticLayerZeroDVN(address(assertingSettlement), address(0), address(0), DEST_EID, 0);
        noReceiveDvn.addSubmitter(submitter);

        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = noReceiveDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.ReceiveUlnNotSet.selector);
        noReceiveDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_pause_blocksAssignJob() public {
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: _defaultPacketHeader(),
            payloadHash: keccak256(abi.encodePacked("payload")),
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        sourceDvn.pause();

        vm.prank(sendUln);
        vm.expectRevert(SymbioticLayerZeroDVN.ContractPaused.selector);
        sourceDvn.assignJob(param, "");
    }

    function test_pause_blocksSubmitProof() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        destinationDvn.pause();

        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.ContractPaused.selector);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_withdraw_revertsForNonOwner() public {
        address payable recipient = payable(makeAddr("recipient"));

        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        destinationDvn.withdraw(recipient);
    }

    function test_unpause_clearsPaused() public {
        destinationDvn.pause();

        destinationDvn.unpause();

        assertFalse(destinationDvn.paused());
    }

    function test_transferOwnership_updatesOwner() public {
        address newOwner = makeAddr("newOwner");

        sourceDvn.transferOwnership(newOwner);

        assertEq(sourceDvn.owner(), newOwner);
    }

    function test_verifyMerkleProof_acceptsLeafRoot() public {
        bytes32 leaf = keccak256(abi.encodePacked("leaf"));
        bytes32[] memory proof = new bytes32[](0);

        bool verified = destinationDvn.verifyMerkleProof(leaf, proof, leaf);

        assertTrue(verified);
    }

    function test_nonReentrant_revertsOnReentry_withDifferentLeaf() public {
        // Setup: Build a 2-leaf Merkle tree with same root but different leaves
        // This proves the reentrancy guard is necessary - without it, leaf2 would be verified
        // in the same transaction via reentrancy

        SettlementStub localSettlement = new SettlementStub();
        localSettlement.setCaptureTimestamp(uint48(block.timestamp));

        ReentrantReceiveUln reentrantUln = new ReentrantReceiveUln();

        SymbioticLayerZeroDVN reentrantDvn =
            new SymbioticLayerZeroDVN(address(localSettlement), address(0), address(reentrantUln), DEST_EID, 0);
        reentrantDvn.addSubmitter(submitter);
        reentrantUln.setDvn(address(reentrantDvn));

        // Create two different leaves from different packet headers
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 leaf1 = reentrantDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);

        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));
        bytes32 leaf2 = reentrantDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        // Build 2-leaf Merkle tree: root = sortedHash(leaf1, leaf2)
        bytes32 root;
        if (uint256(leaf1) < uint256(leaf2)) {
            root = keccak256(abi.encodePacked(leaf1, leaf2));
        } else {
            root = keccak256(abi.encodePacked(leaf2, leaf1));
        }

        // Proofs: proof for leaf1 is [leaf2], proof for leaf2 is [leaf1]
        bytes32[] memory proof1 = new bytes32[](1);
        proof1[0] = leaf2;

        bytes32[] memory proof2 = new bytes32[](1);
        proof2[0] = leaf1;

        // Configure reentrant ULN to attempt submitting leaf2 during reentrancy
        // Empty signature because root will be cached by first submission
        reentrantUln.configureReentry(packetHeader2, payloadHash2, CONFIRMATIONS, proof2, root, "");

        // Build valid signature for the initial submission
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        // First submission: submit leaf1 with valid signature
        // This caches the root, then calls receiveUln.verify()
        // ReentrantReceiveUln attempts to submit leaf2 (different leaf, same cached root)
        vm.prank(submitter);
        reentrantDvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof1, root, signature);

        // Assertions: verify reentrancy was attempted but blocked
        assertTrue(reentrantUln.attempted(), "Reentrancy should have been attempted");
        assertFalse(reentrantUln.reentrySucceeded(), "Reentrancy should have been blocked by guard");

        // leaf1 should be verified (first submission succeeded)
        assertTrue(reentrantDvn.isLeafVerified(leaf1), "leaf1 should be verified");

        // leaf2 should NOT be verified (reentrancy was blocked)
        assertFalse(reentrantDvn.isLeafVerified(leaf2), "leaf2 should NOT be verified (reentrancy blocked)");

        // Root should be cached
        assertTrue(reentrantDvn.isRootVerified(root), "Root should be cached");
    }

    function test_setBaseFee_updatesFee() public {
        uint256 newFee = 0.02 ether;
        sourceDvn.setBaseFee(newFee);

        uint256 fee = sourceDvn.getFee(DEST_EID, CONFIRMATIONS, SENDER, "");
        assertEq(fee, newFee);
    }

    function test_setBaseFee_revertsForNonOwner() public {
        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        sourceDvn.setBaseFee(0.02 ether);
    }

    function test_addSubmitter_revertsForNonOwner() public {
        address newSubmitter = makeAddr("newSubmitter");

        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        destinationDvn.addSubmitter(newSubmitter);
    }

    function test_removeSubmitter_revertsForNonOwner() public {
        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        destinationDvn.removeSubmitter(submitter);
    }

    function test_pause_revertsForNonOwner() public {
        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        destinationDvn.pause();
    }

    function test_unpause_revertsForNonOwner() public {
        destinationDvn.pause();

        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        destinationDvn.unpause();
    }

    function test_transferOwnership_revertsForNonOwner() public {
        address newOwner = makeAddr("newOwner");

        vm.prank(other);
        vm.expectRevert(SymbioticLayerZeroDVN.OnlyOwner.selector);
        sourceDvn.transferOwnership(newOwner);
    }

    function test_transferOwnership_revertsWhenZeroAddress() public {
        vm.expectRevert(SymbioticLayerZeroDVN.ZeroOwner.selector);
        sourceDvn.transferOwnership(address(0));
    }

    function test_submitProof_revertsForNonSubmitter() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(SymbioticLayerZeroDVN.UnauthorizedSubmitter.selector, other));
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_withdraw_transfersBalance() public {
        address payable recipient = payable(makeAddr("recipient"));
        vm.deal(address(destinationDvn), 2 ether);
        uint256 beforeBalance = recipient.balance;

        destinationDvn.withdraw(recipient);

        assertEq(recipient.balance, beforeBalance + 2 ether);
        assertEq(address(destinationDvn).balance, 0);
    }

    function test_withdraw_revertsWhenZeroAddress() public {
        vm.deal(address(destinationDvn), 1 ether);

        vm.expectRevert(SymbioticLayerZeroDVN.ZeroAddress.selector);
        destinationDvn.withdraw(payable(address(0)));
    }

    /// @notice P3 Atomic Rollback Test: Proves that if receiveUln.verify() reverts,
    /// no partial state changes persist (no cached leaf, no cached root, no events).
    /// In Solidity, a revert rolls back ALL state changes in the transaction,
    /// but we explicitly test and document this behavior.
    function test_submitProof_receiveUlnReverts_rollsBackStateCompletely() public {
        // 1. Create RevertingReceiveUln (it reverts by default in verify())
        RevertingReceiveUln revertingReceiveUln = new RevertingReceiveUln();

        // 2. Create SettlementStub configured to return true (signature valid)
        SettlementStub localSettlement = new SettlementStub();
        localSettlement.setCaptureTimestamp(uint48(block.timestamp));
        localSettlement.setSignatureValid(true);

        // 3. Create DVN with the RevertingReceiveUln
        SymbioticLayerZeroDVN revertingDvn = new SymbioticLayerZeroDVN(
            address(localSettlement),
            address(0),
            address(revertingReceiveUln),
            DEST_EID,
            0
        );
        revertingDvn.addSubmitter(submitter);

        // 4. Build a valid submission
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = revertingDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = leaf; // merkleRoot = leaf for single-leaf tree
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        // Verify state is clean before the call
        assertFalse(revertingDvn.isLeafVerified(leaf), "Leaf should not be verified before call");
        assertFalse(revertingDvn.isRootVerified(merkleRoot), "Root should not be verified before call");

        // 5. Attempt submitProof - expect revert with "ReceiveUln verification failed"
        vm.prank(submitter);
        vm.expectRevert("ReceiveUln verification failed");
        revertingDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, signature);

        // 6. Assertions AFTER the revert:
        // The revert should roll back all state changes that happened during submitProof
        // (leaf/root caching happens before receiveUln.verify() is called)
        assertFalse(revertingDvn.isLeafVerified(leaf), "Leaf should NOT be cached after revert");
        assertFalse(revertingDvn.isRootVerified(merkleRoot), "Root should NOT be cached after revert");
    }

    // ============ P0 Settlement calldata correctness tests ============

    function test_submitProof_callsSettlement_withCorrectEpochAndMessageAndProof() public {
        // Create fresh AssertingSettlement and ReceiveUlnStub
        AssertingSettlement assertingSettlement = new AssertingSettlement();
        ReceiveUlnStub localReceiveUln = new ReceiveUlnStub();

        // Create new DVN with AssertingSettlement
        SymbioticLayerZeroDVN dvn =
            new SymbioticLayerZeroDVN(address(assertingSettlement), address(0), address(localReceiveUln), DEST_EID, 0);
        dvn.addSubmitter(submitter);

        // Use a non-trivial epoch (not block.timestamp)
        uint48 epoch = 0x010203040506;

        // Configure settlement with non-default keyTag and threshold
        assertingSettlement.setExpectedEpoch(epoch);
        assertingSettlement.setEpochConfig(epoch, uint48(block.timestamp), 77, 123456);
        assertingSettlement.setVerifyReturnValue(true);

        // Build signature with epoch prefix and BLS proof bytes
        bytes memory blsProofBytes = bytes("BLS_PROOF_BYTES");
        bytes memory signature = abi.encodePacked(epoch, blsProofBytes);

        // Build leaf and merkle root (single-leaf tree)
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = dvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = leaf;

        // Calculate expected values
        bytes32 expectedMessageHash = keccak256(abi.encode(block.chainid, address(dvn), merkleRoot));
        bytes32 expectedProofHash = keccak256(blsProofBytes);

        // Set expectations
        assertingSettlement.setExpectedMessageHash(expectedMessageHash);
        assertingSettlement.setExpectedProofHash(expectedProofHash);

        // Call submitProof as submitter
        vm.prank(submitter);
        dvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, signature);

        // Assertions
        assertTrue(dvn.isLeafVerified(leaf));
        assertTrue(dvn.isRootVerified(merkleRoot));
    }

    function test_submitProof_revertsWhenSettlementReturnsFalse() public {
        // Create fresh AssertingSettlement and ReceiveUlnStub
        AssertingSettlement assertingSettlement = new AssertingSettlement();
        ReceiveUlnStub localReceiveUln = new ReceiveUlnStub();

        // Create new DVN with AssertingSettlement
        SymbioticLayerZeroDVN dvn =
            new SymbioticLayerZeroDVN(address(assertingSettlement), address(0), address(localReceiveUln), DEST_EID, 0);
        dvn.addSubmitter(submitter);

        uint48 epoch = uint48(block.timestamp);

        // Configure settlement with correct params but return false
        assertingSettlement.setExpectedEpoch(epoch);
        assertingSettlement.setEpochConfig(epoch, uint48(block.timestamp), 15, 6600);
        assertingSettlement.setVerifyReturnValue(false); // This should cause revert

        // Build leaf and signature
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = dvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes memory signature = abi.encodePacked(epoch, bytes("sig"));

        // Expect revert with InvalidQuorumSignature
        vm.prank(submitter);
        vm.expectRevert(SymbioticLayerZeroDVN.InvalidQuorumSignature.selector);
        dvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, signature);
    }

    function test_submitProof_revertsWhenSignatureTooShort() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);

        // Test signatures with length 1-5 bytes (less than 6 byte epoch prefix)
        for (uint256 i = 1; i <= 5; i++) {
            bytes memory shortSignature = new bytes(i);
            for (uint256 j = 0; j < i; j++) {
                shortSignature[j] = bytes1(uint8(j + 1));
            }

            vm.prank(submitter);
            vm.expectRevert(); // Should revert (either custom error or out-of-bounds panic)
            destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), leaf, shortSignature);
        }
    }

    /// @notice P1 Cached Root Test: Proves that when a root is already cached,
    /// the second submission does NOT call Settlement (verified by AssertingSettlement
    /// which reverts on any call when configured with shouldRevertOnAnyCall=true).
    function test_submitProof_cachedRoot_skipsSettlementCalls() public {
        // Setup: Create AssertingSettlement and a DVN using it
        AssertingSettlement assertingSettlement = new AssertingSettlement();
        ReceiveUlnStub localReceiveUln = new ReceiveUlnStub();

        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            address(assertingSettlement),
            address(0),
            address(localReceiveUln),
            DEST_EID,
            0
        );
        dvn.addSubmitter(submitter);

        // Build packet headers for two different leaves
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));

        // Compute leaves
        bytes32 leaf1 = dvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = dvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        // Build 2-leaf Merkle tree: root = hash(min(leaf1, leaf2), max(leaf1, leaf2))
        bytes32 root;
        if (leaf1 < leaf2) {
            root = keccak256(abi.encodePacked(leaf1, leaf2));
        } else {
            root = keccak256(abi.encodePacked(leaf2, leaf1));
        }

        // Proofs: proof1 = [leaf2], proof2 = [leaf1]
        bytes32[] memory proof1 = new bytes32[](1);
        proof1[0] = leaf2;
        bytes32[] memory proof2 = new bytes32[](1);
        proof2[0] = leaf1;

        // Configure AssertingSettlement for first submission
        uint48 epoch = uint48(block.timestamp);
        assertingSettlement.setEpochConfig(epoch, uint48(block.timestamp), 15, 6600);
        assertingSettlement.setExpectedEpoch(epoch);
        assertingSettlement.setVerifyReturnValue(true);

        // Compute expected message hash for AssertingSettlement validation
        bytes32 expectedMessageHash = keccak256(abi.encode(block.chainid, address(dvn), root));
        assertingSettlement.setExpectedMessageHash(expectedMessageHash);

        // Build signature: epoch (6 bytes) + BLS signature
        bytes memory blsSignature = bytes("valid_bls_sig");
        bytes memory signature = abi.encodePacked(epoch, blsSignature);
        assertingSettlement.setExpectedProofHash(keccak256(blsSignature));

        // First submission (leaf1 with signature) - should succeed and cache the root
        vm.prank(submitter);
        dvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof1, root, signature);

        // Verify first submission succeeded
        assertTrue(dvn.isLeafVerified(leaf1), "leaf1 should be verified");
        assertTrue(dvn.isRootVerified(root), "root should be cached");

        // Reconfigure AssertingSettlement: if Settlement is called, test fails
        assertingSettlement.setShouldRevertOnAnyCall(true);

        // Second submission (leaf2 with empty signature)
        // The cached root path allows empty signature
        // If Settlement is called, AssertingSettlement reverts and test fails
        vm.prank(submitter);
        dvn.submitProof(packetHeader2, payloadHash2, CONFIRMATIONS, proof2, root, "");

        // Assertions: Both leaves verified, root verified, second submission succeeded
        assertTrue(dvn.isLeafVerified(leaf1), "leaf1 should still be verified");
        assertTrue(dvn.isLeafVerified(leaf2), "leaf2 should be verified");
        assertTrue(dvn.isRootVerified(root), "root should still be cached");

        // Verify receiveUln was called for both leaves
        assertEq(localReceiveUln.verifyCalls(), 2, "receiveUln should have been called twice");
    }

    // ============ P5 Event assertion tests ============

    /// @notice P5 Test 1: Verify JobAssigned event has correct fields extracted from packetHeader
    /// @dev Tests that:
    /// - srcEid is extracted correctly from packetHeader bytes [9:13]
    /// - nonce is extracted correctly from packetHeader bytes [1:9]
    /// - receiver is extracted from packetHeader bytes [49:81]
    /// - guid = keccak256(packetHeader)
    /// - fee = baseFee
    function test_assignJob_emitsJobAssigned_withCorrectFields() public {
        // Use specific known values for nonce to verify extraction
        uint64 knownNonce = 0x0102030405060708;

        bytes memory packetHeader = _buildPacketHeader(1, knownNonce, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash = keccak256(abi.encodePacked("test_payload"));
        bytes memory options = bytes("test_options");

        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: DEST_EID,
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: CONFIRMATIONS,
            sender: SENDER
        });

        // Expect JobAssigned event with all indexed + data fields
        // The DVN extracts: srcEid from bytes [9:13], nonce from bytes [1:9], receiver from bytes [49:81]
        vm.expectEmit(true, true, true, true);
        emit SymbioticLayerZeroDVN.JobAssigned(
            keccak256(packetHeader),                // guid = keccak256(packetHeader)
            SOURCE_EID,                             // srcEid (extracted from packetHeader[9:13])
            DEST_EID,                               // dstEid
            SENDER,                                 // sender
            bytes32(uint256(uint160(RECEIVER))),    // receiver (extracted from packetHeader[49:81])
            payloadHash,                            // payloadHash
            packetHeader,                           // packetHeader
            CONFIRMATIONS,                          // confirmations
            knownNonce,                             // nonce (extracted from packetHeader[1:9])
            options,                                // options
            BASE_FEE                                // fee = baseFee
        );

        vm.prank(sendUln);
        sourceDvn.assignJob(param, options);
    }

    /// @notice P5 Test 2: Verify MerkleRootCached event has correct epoch and root
    function test_submitProof_emitsMerkleRootCached_withCorrectEpochAndRoot() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = leaf; // Single-leaf tree

        // Use a specific epoch value
        uint48 epoch = 0x010203040506;
        settlement.setCaptureTimestamp(uint48(block.timestamp));

        // Build signature with the specific epoch
        bytes memory signature = abi.encodePacked(epoch, bytes("sig"));

        // Expect MerkleRootCached event with correct root and epoch
        vm.expectEmit(true, true, true, true);
        emit SymbioticLayerZeroDVN.MerkleRootCached(merkleRoot, epoch);

        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, signature);
    }

    /// @notice P5 Test 3: Verify VerificationSubmitted event has correct values
    function test_submitProof_emitsVerificationSubmitted_withCorrectValues() public {
        bytes memory packetHeader = _defaultPacketHeader();
        bytes32 payloadHash = keccak256(abi.encodePacked("payload"));
        bytes32 leaf = destinationDvn.computeLeaf(packetHeader, payloadHash, CONFIRMATIONS);
        bytes32 merkleRoot = leaf; // Single-leaf tree
        bytes memory signature = _buildSignature(uint48(block.timestamp));

        // Expect VerificationSubmitted event with correct values
        vm.expectEmit(true, true, true, true);
        emit SymbioticLayerZeroDVN.VerificationSubmitted(leaf, merkleRoot, CONFIRMATIONS);

        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader, payloadHash, CONFIRMATIONS, new bytes32[](0), merkleRoot, signature);
    }

    /// @notice P5 Test 4: Verify that cached root does NOT emit MerkleRootCached
    function test_submitProof_cachedRoot_doesNotEmitMerkleRootCached() public {
        // Build packet headers for two different leaves
        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256(abi.encodePacked("payload1"));
        bytes32 payloadHash2 = keccak256(abi.encodePacked("payload2"));

        // Compute leaves
        bytes32 leaf1 = destinationDvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        bytes32 leaf2 = destinationDvn.computeLeaf(packetHeader2, payloadHash2, CONFIRMATIONS);

        // Build 2-leaf Merkle tree
        bytes32 root;
        if (leaf1 < leaf2) {
            root = keccak256(abi.encodePacked(leaf1, leaf2));
        } else {
            root = keccak256(abi.encodePacked(leaf2, leaf1));
        }

        // Proofs
        bytes32[] memory proof1 = new bytes32[](1);
        proof1[0] = leaf2;
        bytes32[] memory proof2 = new bytes32[](1);
        proof2[0] = leaf1;

        bytes memory signature = _buildSignature(uint48(block.timestamp));

        // First submission - caches the root
        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof1, root, signature);

        // Verify root is cached
        assertTrue(destinationDvn.isRootVerified(root), "Root should be cached after first submission");

        // Record logs for second submission
        vm.recordLogs();

        // Second submission - uses cached root, no signature needed
        vm.prank(submitter);
        destinationDvn.submitProof(packetHeader2, payloadHash2, CONFIRMATIONS, proof2, root, "");

        // Get recorded logs
        Vm.Log[] memory logs = vm.getRecordedLogs();

        // Check that MerkleRootCached was NOT emitted
        bytes32 merkleRootCachedTopic = keccak256("MerkleRootCached(bytes32,uint48)");
        bool merkleRootCachedEmitted = false;
        bool verificationSubmittedEmitted = false;
        bytes32 verificationSubmittedTopic = keccak256("VerificationSubmitted(bytes32,bytes32,uint64)");

        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == merkleRootCachedTopic) {
                merkleRootCachedEmitted = true;
            }
            if (logs[i].topics[0] == verificationSubmittedTopic) {
                verificationSubmittedEmitted = true;
            }
        }

        assertFalse(merkleRootCachedEmitted, "MerkleRootCached should NOT be emitted for cached root");
        assertTrue(verificationSubmittedEmitted, "VerificationSubmitted should still be emitted");

        // Verify leaf2 was verified
        assertTrue(destinationDvn.isLeafVerified(leaf2), "leaf2 should be verified");
    }

    function _defaultPacketHeader() internal pure returns (bytes memory) {
        return _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
    }

    function _buildPacketHeader(
        uint8 version,
        uint64 nonce,
        uint32 srcEid,
        address sender,
        uint32 dstEid,
        address receiver
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            version,
            nonce,
            srcEid,
            bytes32(uint256(uint160(sender))),
            dstEid,
            bytes32(uint256(uint160(receiver)))
        );
    }

    function _buildSignature(uint48 epoch) internal pure returns (bytes memory) {
        return abi.encodePacked(epoch, bytes("sig"));
    }
}
