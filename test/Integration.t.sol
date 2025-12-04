// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import "forge-std/Test.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @title Integration Tests for SymbioticLayerZeroDVN
/// @notice Tests the complete flow from job assignment to verification
contract IntegrationTest is Test {
    SymbioticLayerZeroDVN sourceDVN;
    SymbioticLayerZeroDVN destDVN;
    MockSettlement mockSettlement;
    MockReceiveUln mockReceiveUln;

    uint256 constant BASE_FEE = 0.001 ether;
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    address deployer = address(0x1);
    address operator = address(0x2);

    function setUp() public {
        vm.deal(deployer, 100 ether);
        vm.deal(operator, 100 ether);

        vm.startPrank(deployer);

        // Deploy mock settlement that always verifies
        mockSettlement = new MockSettlement();

        // Deploy source chain DVN (no Settlement needed)
        sourceDVN = new SymbioticLayerZeroDVN(address(0), BASE_FEE);

        // Deploy destination chain DVN with mock Settlement
        destDVN = new SymbioticLayerZeroDVN(address(mockSettlement), BASE_FEE);

        // Deploy mock ReceiveUln and configure DVN
        mockReceiveUln = new MockReceiveUln();
        destDVN.setReceiveUln(address(mockReceiveUln));

        vm.stopPrank();
    }

    /// @notice Test the complete flow: assign job → submit verification
    function testCompleteFlow() public {
        // 1. Assign a job on source chain DVN
        bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, 1);
        bytes32 payloadHash = keccak256("Hello LayerZero!");
        uint64 confirmations = 15;

        vm.prank(operator);
        bytes32 jobId = sourceDVN.assignJob{value: BASE_FEE}(
            DEST_EID, packetHeader, payloadHash, confirmations, operator
        );

        // Verify job was created
        assertEq(
            uint8(sourceDVN.getJobStatus(jobId)),
            uint8(SymbioticLayerZeroDVN.JobStatus.PENDING)
        );

        // 2. Submit verification on destination chain DVN
        // In production, this is done by the off-chain worker
        uint48 epoch = 1;
        bytes memory proof = hex"deadbeefcafe"; // Mock proof

        vm.prank(operator);
        destDVN.submitVerification(packetHeader, payloadHash, confirmations, epoch, proof);

        // 3. Verify ReceiveUln was called
        assertTrue(mockReceiveUln.verifyCalled());
        assertEq(mockReceiveUln.lastPayloadHash(), payloadHash);
        assertEq(mockReceiveUln.lastConfirmations(), confirmations);
    }

    /// @notice Test job assignment emits correct event
    function testJobAssignedEvent() public {
        bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, 1);
        bytes32 payloadHash = keccak256("test message");
        uint64 confirmations = 10;

        bytes32 expectedJobId = keccak256(
            abi.encode(block.chainid, DEST_EID, packetHeader, payloadHash)
        );

        vm.expectEmit(true, true, false, true);
        emit SymbioticLayerZeroDVN.JobAssigned(
            expectedJobId,
            DEST_EID,
            payloadHash,
            operator,
            packetHeader,
            confirmations
        );

        vm.prank(operator);
        sourceDVN.assignJob{value: BASE_FEE}(
            DEST_EID, packetHeader, payloadHash, confirmations, operator
        );
    }

    /// @notice Test verification emits correct event
    function testVerificationSubmittedEvent() public {
        bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, 1);
        bytes32 payloadHash = keccak256("test message");
        uint64 confirmations = 10;
        uint48 epoch = 1;
        bytes memory proof = hex"deadbeef";

        bytes32 expectedMessageHash = keccak256(abi.encode(packetHeader, payloadHash));

        vm.expectEmit(true, false, false, true);
        emit SymbioticLayerZeroDVN.VerificationSubmitted(expectedMessageHash, epoch, confirmations);

        vm.prank(operator);
        destDVN.submitVerification(packetHeader, payloadHash, confirmations, epoch, proof);
    }

    /// @notice Test that verification fails without ReceiveUln configured
    function testVerificationFailsWithoutReceiveUln() public {
        // Create a new DVN without ReceiveUln configured
        vm.prank(deployer);
        SymbioticLayerZeroDVN dvnNoReceiveUln = new SymbioticLayerZeroDVN(address(mockSettlement), BASE_FEE);

        bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, 1);
        bytes32 payloadHash = keccak256("test");

        vm.expectRevert("ReceiveUln not set");
        vm.prank(operator);
        dvnNoReceiveUln.submitVerification(packetHeader, payloadHash, 10, 1, hex"deadbeef");
    }

    /// @notice Test fee handling
    function testFeeHandling() public {
        bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, 1);
        bytes32 payloadHash = keccak256("test");

        uint256 balanceBefore = operator.balance;

        // Send exact fee required
        vm.prank(operator);
        sourceDVN.assignJob{value: BASE_FEE}(
            DEST_EID, packetHeader, payloadHash, 10, operator
        );

        // Fee should be deducted
        uint256 balanceAfter = operator.balance;
        assertEq(balanceBefore - balanceAfter, BASE_FEE);

        // DVN should have the fee
        assertEq(address(sourceDVN).balance, BASE_FEE);
    }

    /// @notice Test fee withdrawal
    function testFeeWithdrawal() public {
        // Assign a job to collect fees
        vm.prank(operator);
        sourceDVN.assignJob{value: BASE_FEE}(
            DEST_EID, hex"01", bytes32(uint256(1)), 10, operator
        );

        uint256 dvnBalance = address(sourceDVN).balance;
        assertEq(dvnBalance, BASE_FEE);

        // Withdraw fees
        address payable recipient = payable(address(0x999));
        vm.prank(deployer);
        sourceDVN.withdraw(recipient);

        assertEq(address(sourceDVN).balance, 0);
        assertEq(recipient.balance, BASE_FEE);
    }

    /// @notice Test multiple jobs can be processed
    function testMultipleJobs() public {
        for (uint256 i = 0; i < 5; i++) {
            bytes memory packetHeader = _createMockPacketHeader(SOURCE_EID, DEST_EID, uint64(i + 1));
            bytes32 payloadHash = keccak256(abi.encode("message", i));

            // Assign job on source
            vm.prank(operator);
            sourceDVN.assignJob{value: BASE_FEE}(
                DEST_EID, packetHeader, payloadHash, 10, operator
            );

            // Submit verification on destination
            vm.prank(operator);
            destDVN.submitVerification(packetHeader, payloadHash, 10, 1, hex"deadbeef");
        }

        // All verifications should have called ReceiveUln
        assertEq(mockReceiveUln.verifyCallCount(), 5);
    }

    /// @notice Create a mock LayerZero packet header
    function _createMockPacketHeader(
        uint32 srcEid,
        uint32 dstEid,
        uint64 nonce
    ) internal pure returns (bytes memory) {
        // Simplified packet header format
        // Real format: version (1) + nonce (8) + srcEid (4) + sender (32) + dstEid (4) + receiver (32)
        return abi.encodePacked(
            uint8(1), // version
            nonce,
            srcEid,
            bytes32(uint256(0x1111)), // sender
            dstEid,
            bytes32(uint256(0x2222)) // receiver
        );
    }
}

/// @notice Mock ReceiveUln for testing
contract MockReceiveUln {
    bool public verifyCalled;
    bytes public lastPacketHeader;
    bytes32 public lastPayloadHash;
    uint64 public lastConfirmations;
    uint256 public verifyCallCount;

    function verify(bytes calldata _packetHeader, bytes32 _payloadHash, uint64 _confirmations) external {
        verifyCalled = true;
        lastPacketHeader = _packetHeader;
        lastPayloadHash = _payloadHash;
        lastConfirmations = _confirmations;
        verifyCallCount++;
    }
}

/// @notice Mock Settlement that always verifies signatures
contract MockSettlement {
    function getCaptureTimestampFromValSetHeaderAt(uint48) external pure returns (uint48) {
        return 0; // No next epoch
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
