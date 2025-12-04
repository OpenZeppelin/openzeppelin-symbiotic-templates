// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "@symbioticfi/relay-contracts/interfaces/modules/settlement/ISettlement.sol";
import {ILayerZeroDVN} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/interfaces/ILayerZeroDVN.sol";
import {IReceiveUlnE2} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/interfaces/IReceiveUlnE2.sol";

/// @title SymbioticLayerZeroDVN
/// @notice A DVN (Decentralized Verifier Network) for LayerZero secured by Symbiotic
/// @dev Implements ILayerZeroDVN interface and uses Symbiotic quorum verification
contract SymbioticLayerZeroDVN is ILayerZeroDVN {
    error AlreadyVerified();
    error JobNotFound();
    error InvalidQuorumSignature();
    error InvalidVerifyingEpoch();
    error InsufficientFee();

    enum JobStatus {
        NOT_FOUND,
        PENDING,
        VERIFIED,
        EXPIRED
    }

    struct Job {
        uint32 dstEid;
        bytes packetHeader;
        bytes32 payloadHash;
        uint64 confirmations;
        address sender;
        uint48 createdAt;
        bool verified;
    }

    /// @notice Mapping of dstEid to ReceiveUln address on destination chain
    /// @dev In a real multi-chain setup, this would map to addresses on remote chains
    /// For local testing, this maps to local addresses
    struct DstConfig {
        address receiveUln;
        bool active;
    }

    event JobAssigned(
        bytes32 indexed jobId,
        uint32 indexed dstEid,
        bytes32 payloadHash,
        address sender,
        bytes packetHeader,
        uint64 confirmations
    );
    event JobVerified(bytes32 indexed jobId, uint48 epoch);
    event VerificationSubmitted(bytes32 indexed packetHash, uint48 epoch, uint64 confirmations);
    event DstConfigSet(uint32 indexed dstEid, address receiveUln);
    event ReceiveUlnSet(address receiveUln);

    /// @notice Expiry window for jobs (in seconds)
    uint32 public constant JOB_EXPIRY = 3600; // 1 hour

    /// @notice Base fee for verification (can be made configurable per destination)
    uint256 public baseFee;

    /// @notice Symbiotic settlement contract for quorum verification
    ISettlement public settlement;

    /// @notice ReceiveUln302 address on this chain (for destination chain DVN)
    address public receiveUln;

    /// @notice Owner of the DVN (for admin functions)
    address public owner;

    /// @notice All jobs indexed by jobId
    mapping(bytes32 => Job) public jobs;

    /// @notice Destination chain configurations
    mapping(uint32 => DstConfig) public dstConfigs;

    modifier onlyOwner() {
        require(msg.sender == owner, "Only owner");
        _;
    }

    constructor(address _settlement, uint256 _baseFee) {
        settlement = ISettlement(_settlement);
        baseFee = _baseFee;
        owner = msg.sender;
    }

    /// @notice Set the ReceiveUln address for a destination chain (legacy)
    /// @param dstEid Destination endpoint ID
    /// @param _receiveUln Address of ReceiveUln302 on that chain
    function setDstConfig(uint32 dstEid, address _receiveUln) external onlyOwner {
        dstConfigs[dstEid] = DstConfig({receiveUln: _receiveUln, active: true});
        emit DstConfigSet(dstEid, _receiveUln);
    }

    /// @notice Set the ReceiveUln address for this chain (destination chain setup)
    /// @param _receiveUln Address of ReceiveUln302 on this chain
    function setReceiveUln(address _receiveUln) external onlyOwner {
        receiveUln = _receiveUln;
        emit ReceiveUlnSet(_receiveUln);
    }

    /// @notice Get the status of a job
    function getJobStatus(bytes32 jobId) public view returns (JobStatus) {
        Job storage job = jobs[jobId];

        if (job.createdAt == 0) {
            return JobStatus.NOT_FOUND;
        }

        if (job.verified) {
            return JobStatus.VERIFIED;
        }

        if (block.timestamp > job.createdAt + JOB_EXPIRY) {
            return JobStatus.EXPIRED;
        }

        return JobStatus.PENDING;
    }

    /// @notice Called by LayerZero SendUln302 to assign a verification job
    /// @dev Implements ILayerZeroDVN.assignJob
    /// @dev Note: LayerZero protocol pays DVN fees separately, not via msg.value in assignJob
    /// @param _param Job parameters (dstEid, packetHeader, payloadHash, confirmations, sender)
    /// @param _options Optional parameters (unused in this implementation)
    /// @return fee The fee charged for this job
    function assignJob(
        AssignJobParam calldata _param,
        bytes calldata _options
    ) external payable override returns (uint256 fee) {
        fee = getFee(_param.dstEid, _param.confirmations, _param.sender, _options);

        // Note: LayerZero protocol handles DVN fee payment separately after this call
        // via a transfer from the messaging library. We don't require msg.value here.

        bytes32 jobId = keccak256(abi.encode(block.chainid, _param.dstEid, _param.packetHeader, _param.payloadHash));

        jobs[jobId] = Job({
            dstEid: _param.dstEid,
            packetHeader: _param.packetHeader,
            payloadHash: _param.payloadHash,
            confirmations: _param.confirmations,
            sender: _param.sender,
            createdAt: uint48(block.timestamp),
            verified: false
        });

        emit JobAssigned(
            jobId,
            _param.dstEid,
            _param.payloadHash,
            _param.sender,
            _param.packetHeader,
            _param.confirmations
        );

        return fee;
    }

    /// @notice Legacy assignJob for backwards compatibility and testing
    /// @dev Keeps the original function signature for existing tests
    function assignJob(
        uint32 dstEid,
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations,
        address sender
    ) external payable returns (bytes32 jobId) {
        if (msg.value < baseFee) {
            revert InsufficientFee();
        }

        jobId = keccak256(abi.encode(block.chainid, dstEid, packetHeader, payloadHash));

        jobs[jobId] = Job({
            dstEid: dstEid,
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: confirmations,
            sender: sender,
            createdAt: uint48(block.timestamp),
            verified: false
        });

        emit JobAssigned(jobId, dstEid, payloadHash, sender, packetHeader, confirmations);

        return jobId;
    }

    /// @notice Submit verification with Symbiotic quorum proof (destination chain, full packet data)
    /// @dev Called by off-chain DVN worker with packet data received from source chain events
    /// @param packetHeader The LayerZero packet header
    /// @param payloadHash Hash of the message payload
    /// @param confirmations Number of block confirmations
    /// @param epoch The Symbiotic epoch used for signing
    /// @param proof The aggregated quorum signature proof
    function submitVerification(
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations,
        uint48 epoch,
        bytes calldata proof
    ) external {
        // Build the message that was signed: keccak256(packetHeader, payloadHash)
        bytes32 messageHash = keccak256(abi.encode(packetHeader, payloadHash));
        bytes memory message = abi.encode(messageHash);

        // Verify the quorum signature via Symbiotic Settlement
        if (
            !settlement.verifyQuorumSigAt(
                message,
                settlement.getRequiredKeyTagFromValSetHeaderAt(epoch),
                settlement.getQuorumThresholdFromValSetHeaderAt(epoch),
                proof,
                epoch,
                new bytes(0)
            )
        ) {
            revert InvalidQuorumSignature();
        }

        emit VerificationSubmitted(messageHash, epoch, confirmations);

        // Call verify on ReceiveUln302 to notify LayerZero this DVN attests to the message
        require(receiveUln != address(0), "ReceiveUln not set");
        IReceiveUlnE2(receiveUln).verify(packetHeader, payloadHash, confirmations);
    }

    /// @notice Submit verification with Symbiotic quorum proof (legacy, jobId-based)
    /// @dev After verification, optionally calls ReceiveUln.verify() on destination chain
    /// @param jobId The job to verify
    /// @param epoch The Symbiotic epoch used for signing
    /// @param proof The aggregated quorum signature proof
    function submitVerification(bytes32 jobId, uint48 epoch, bytes calldata proof) external {
        Job storage job = jobs[jobId];

        if (job.createdAt == 0) {
            revert JobNotFound();
        }

        if (job.verified) {
            revert AlreadyVerified();
        }

        // Verify that the verifying epoch is not stale
        uint48 nextEpochCaptureTimestamp = settlement.getCaptureTimestampFromValSetHeaderAt(epoch + 1);
        if (nextEpochCaptureTimestamp > 0 && block.timestamp >= nextEpochCaptureTimestamp + JOB_EXPIRY) {
            revert InvalidVerifyingEpoch();
        }

        // Build the message that was signed: (jobId, payloadHash)
        bytes memory message = abi.encode(keccak256(abi.encode(jobId, job.payloadHash)));

        // Verify the quorum signature via Symbiotic Settlement
        if (
            !settlement.verifyQuorumSigAt(
                message,
                settlement.getRequiredKeyTagFromValSetHeaderAt(epoch),
                settlement.getQuorumThresholdFromValSetHeaderAt(epoch),
                proof,
                epoch,
                new bytes(0)
            )
        ) {
            revert InvalidQuorumSignature();
        }

        job.verified = true;

        emit JobVerified(jobId, epoch);

        // If we have a ReceiveUln configured for this destination, call verify
        // Note: In a real multi-chain setup, this call would happen on the destination chain
        // via the off-chain DVN node, not in this contract directly
        DstConfig storage config = dstConfigs[job.dstEid];
        if (config.active && config.receiveUln != address(0)) {
            // Call verify on the ReceiveUln
            // This notifies LayerZero that this DVN has verified the message
            IReceiveUlnE2(config.receiveUln).verify(job.packetHeader, job.payloadHash, job.confirmations);
        }
    }

    /// @notice Get the fee required for verification
    /// @dev Implements ILayerZeroDVN.getFee
    function getFee(
        uint32, /* dstEid */
        uint64, /* confirmations */
        address, /* sender */
        bytes calldata /* options */
    ) public view override returns (uint256) {
        return baseFee;
    }

    /// @notice Withdraw collected fees
    function withdraw(address payable to) external onlyOwner {
        to.transfer(address(this).balance);
    }

    /// @notice Update base fee
    function setBaseFee(uint256 _baseFee) external onlyOwner {
        baseFee = _baseFee;
    }

    /// @notice Transfer ownership
    function transferOwnership(address newOwner) external onlyOwner {
        owner = newOwner;
    }
}
