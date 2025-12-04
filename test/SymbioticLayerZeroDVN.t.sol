// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import "forge-std/Test.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

contract MockSettlement {
    function getCaptureTimestampFromValSetHeaderAt(uint48) external pure returns (uint48) {
        return 0; // No next epoch yet
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure returns (uint8) {
        return 15; // BLS-BN254
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure returns (uint256) {
        return 6667; // 66.67%
    }

    function verifyQuorumSigAt(
        bytes memory,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external pure returns (bool) {
        return true; // Always valid for testing
    }
}

contract SymbioticLayerZeroDVNTest is Test {
    SymbioticLayerZeroDVN dvn;
    MockSettlement settlement;

    uint256 constant BASE_FEE = 0.001 ether;

    function setUp() public {
        settlement = new MockSettlement();
        dvn = new SymbioticLayerZeroDVN(address(settlement), BASE_FEE);
    }

    function testAssignJobStoresJob() public {
        uint32 dstEid = 101;
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));
        uint64 confirmations = 2;
        address sender = address(this);

        dvn.assignJob{value: BASE_FEE}(dstEid, packetHeader, payloadHash, confirmations, sender);

        bytes32 jobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));

        (
            uint32 storedDstEid,
            bytes memory storedPacketHeader,
            bytes32 storedPayloadHash,
            uint64 storedConfirmations,
            address storedSender,
            uint48 createdAt,
            bool verified
        ) = dvn.jobs(jobId);

        assertEq(storedDstEid, dstEid);
        assertEq(storedPayloadHash, payloadHash);
        assertEq(storedConfirmations, confirmations);
        assertEq(storedSender, sender);
        assertGt(createdAt, 0);
        assertFalse(verified);
    }

    function testAssignJobEmitsEvent() public {
        uint32 dstEid = 101;
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));

        bytes32 expectedJobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));

        vm.expectEmit(true, true, false, true);
        emit SymbioticLayerZeroDVN.JobAssigned(
            expectedJobId,
            dstEid,
            payloadHash,
            address(this),
            packetHeader,
            2 // confirmations
        );

        dvn.assignJob{value: BASE_FEE}(dstEid, packetHeader, payloadHash, 2, address(this));
    }

    function testAssignJobRevertsOnInsufficientFee() public {
        vm.expectRevert(SymbioticLayerZeroDVN.InsufficientFee.selector);
        dvn.assignJob{value: 0}(101, hex"01", bytes32(0), 2, address(this));
    }

    function testGetFee() public view {
        uint256 fee = dvn.getFee(101, 2, address(this), "");
        assertEq(fee, BASE_FEE);
    }

    function testGetJobStatus() public {
        bytes32 fakeJobId = bytes32(uint256(1));
        assertEq(uint8(dvn.getJobStatus(fakeJobId)), uint8(SymbioticLayerZeroDVN.JobStatus.NOT_FOUND));

        // Create a job
        uint32 dstEid = 101;
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));
        dvn.assignJob{value: BASE_FEE}(dstEid, packetHeader, payloadHash, 2, address(this));

        bytes32 jobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));
        assertEq(uint8(dvn.getJobStatus(jobId)), uint8(SymbioticLayerZeroDVN.JobStatus.PENDING));
    }

    function testSubmitVerification() public {
        // Create a job
        uint32 dstEid = 101;
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));
        dvn.assignJob{value: BASE_FEE}(dstEid, packetHeader, payloadHash, 2, address(this));

        bytes32 jobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));

        // Submit verification with mock proof
        uint48 epoch = 1;
        bytes memory proof = hex"deadbeef";

        vm.expectEmit(true, false, false, true);
        emit SymbioticLayerZeroDVN.JobVerified(jobId, epoch);

        dvn.submitVerification(jobId, epoch, proof);

        // Check job is now verified
        assertEq(uint8(dvn.getJobStatus(jobId)), uint8(SymbioticLayerZeroDVN.JobStatus.VERIFIED));
    }

    function testSubmitVerificationRevertsIfAlreadyVerified() public {
        // Create and verify a job
        uint32 dstEid = 101;
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));
        dvn.assignJob{value: BASE_FEE}(dstEid, packetHeader, payloadHash, 2, address(this));

        bytes32 jobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));
        dvn.submitVerification(jobId, 1, hex"deadbeef");

        // Try to verify again
        vm.expectRevert(SymbioticLayerZeroDVN.AlreadyVerified.selector);
        dvn.submitVerification(jobId, 1, hex"deadbeef");
    }

    function testSubmitVerificationRevertsIfJobNotFound() public {
        bytes32 fakeJobId = bytes32(uint256(1));
        vm.expectRevert(SymbioticLayerZeroDVN.JobNotFound.selector);
        dvn.submitVerification(fakeJobId, 1, hex"deadbeef");
    }

    function testSubmitVerificationWithPacketData() public {
        // Set up mock ReceiveUln
        MockReceiveUln mockReceiveUln = new MockReceiveUln();
        dvn.setReceiveUln(address(mockReceiveUln));

        // Packet data (as would come from JobAssigned event on source chain)
        bytes memory packetHeader = hex"01000000000000000100007a6900000000000000000000000000000000000000000000000000000000000000000000000000007a6a";
        bytes32 payloadHash = bytes32(uint256(0xdeadbeef));
        uint64 confirmations = 15;
        uint48 epoch = 1;
        bytes memory proof = hex"deadbeefcafe";

        // Expected message hash
        bytes32 expectedMessageHash = keccak256(abi.encode(packetHeader, payloadHash));

        vm.expectEmit(true, false, false, true);
        emit SymbioticLayerZeroDVN.VerificationSubmitted(expectedMessageHash, epoch, confirmations);

        // Submit verification with full packet data (destination chain pattern)
        dvn.submitVerification(packetHeader, payloadHash, confirmations, epoch, proof);

        // Verify ReceiveUln was called
        assertTrue(mockReceiveUln.verifyCalled());
        assertEq(mockReceiveUln.lastPayloadHash(), payloadHash);
        assertEq(mockReceiveUln.lastConfirmations(), confirmations);
    }

    function testSubmitVerificationWithPacketDataRevertsIfReceiveUlnNotSet() public {
        bytes memory packetHeader = hex"01";
        bytes32 payloadHash = bytes32(uint256(0xdead));

        vm.expectRevert("ReceiveUln not set");
        dvn.submitVerification(packetHeader, payloadHash, 2, 1, hex"deadbeef");
    }
}

contract MockReceiveUln {
    bool public verifyCalled;
    bytes public lastPacketHeader;
    bytes32 public lastPayloadHash;
    uint64 public lastConfirmations;

    function verify(bytes calldata _packetHeader, bytes32 _payloadHash, uint64 _confirmations) external {
        verifyCalled = true;
        lastPacketHeader = _packetHeader;
        lastPayloadHash = _payloadHash;
        lastConfirmations = _confirmations;
    }
}
