// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "../interfaces/ISettlement.sol";
import {ILayerZeroDVN} from "./interfaces/ILayerZeroDVN.sol";
import {IReceiveUlnE2} from "./interfaces/IReceiveUlnE2.sol";
import {MerkleProof} from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";

/// @title SymbioticLayerZeroDVN
/// @author Symbiotic
/// @notice A DVN (Decentralized Verifier Network) for LayerZero secured by Symbiotic with Merkle tree batching
/// @dev Implements ILayerZeroDVN interface and uses Symbiotic BLS quorum verification
/// @dev Single contract deployed on both source and destination chains with different active functions
/// @dev Features: Merkle tree batching, root caching, authorized submitters whitelist
contract SymbioticLayerZeroDVN is ILayerZeroDVN {
    // ============ Errors ============

    /// @notice Thrown when caller is not the SendUln302 contract
    error OnlySendUln();

    /// @notice Thrown when caller is not the owner
    error OnlyOwner();

    /// @notice Thrown when caller is not an authorized submitter
    /// @param caller The address that attempted to call the function
    error UnauthorizedSubmitter(address caller);

    /// @notice Thrown when BLS quorum signature verification fails
    error InvalidQuorumSignature();

    /// @notice Thrown when packet header is malformed (not 81 bytes)
    error InvalidPacketHeader();

    /// @notice Thrown when packet header version is unsupported
    error InvalidPacketVersion();

    /// @notice Thrown when packet destination doesn't match local endpoint ID
    error WrongDestinationChain();

    /// @notice Thrown when assignJob dstEid doesn't match the packet header destination
    /// @param paramDstEid Destination endpoint ID passed in the job parameters
    /// @param headerDstEid Destination endpoint ID encoded in the packet header
    error PacketDstEidMismatch(uint32 paramDstEid, uint32 headerDstEid);

    /// @notice Thrown when assignJob sender parameter does not match packet header
    error SenderMismatch();

    /// @notice Thrown when attempting to verify an already verified leaf
    error AlreadyVerified();

    /// @notice Thrown when epoch is too old to be valid
    error EpochTooStale();

    /// @notice Thrown when a destination DVN is deployed without an epoch validity window
    error InvalidEpochValidity();

    /// @notice Thrown when epoch doesn't exist in Settlement
    error InvalidEpoch();

    /// @notice Thrown when Merkle proof exceeds maximum allowed size
    error ProofTooLarge();

    /// @notice Thrown when batch submission exceeds maximum allowed size
    error BatchTooLarge();

    /// @notice Thrown when quorum signature exceeds maximum allowed size
    error SignatureTooLarge();

    /// @notice Thrown when receiveUln is not configured (destination chain only)
    error ReceiveUlnNotSet();

    /// @notice Thrown when contract is paused
    error ContractPaused();

    /// @notice Thrown when reentrancy is detected
    error ReentrancyGuardReentrant();

    /// @notice Thrown when Merkle proof verification fails
    error InvalidMerkleProof();

    /// @notice Thrown when submitter is already authorized
    error SubmitterAlreadyAuthorized();

    /// @notice Thrown when submitter is not authorized
    error SubmitterNotAuthorized();

    /// @notice Thrown when signature is required but not provided for uncached root
    error SignatureRequired();

    /// @notice Thrown when signature is below minimum format (epoch + quorum proof)
    error SignatureTooShort();

    /// @notice Thrown when batch submission is called with no proofs
    error EmptyBatch();

    /// @notice Thrown when ETH is sent to assignJob (DVN does not custody fees)
    error NoFeeAccepted();

    /// @notice Thrown when base fee update does not change the value
    error BaseFeeUnchanged();

    /// @notice Thrown when new owner is the zero address
    error ZeroOwner();

    /// @notice Thrown when ownership transfer target is the current owner
    error OwnerUnchanged();

    /// @notice Thrown when caller is not the pending owner
    error OnlyPendingOwner();

    /// @notice Thrown when withdraw recipient is the zero address
    error ZeroAddress();

    /// @notice Thrown when ETH transfer fails
    error WithdrawFailed();

    /// @notice Thrown when local endpoint ID is zero
    error InvalidLocalEid();

    /// @notice Thrown when neither source nor destination role is configured
    error InvalidRoleConfiguration();

    /// @notice Thrown when destination role is configured without settlement
    error SettlementRequired();

    // ============ Events ============

    /// @notice Emitted when a verification job is assigned on source chain (Symbiotic spec - 11 fields)
    /// @param guid Globally unique identifier for this message
    /// @param srcEid Source endpoint ID
    /// @param dstEid Destination endpoint ID
    /// @param sender Address of the sender on source chain
    /// @param receiver Address of the receiver on destination chain (as bytes32)
    /// @param payloadHash Hash of the message payload
    /// @param packetHeader The LayerZero packet header
    /// @param confirmations Required block confirmations
    /// @param nonce Message nonce
    /// @param options Optional parameters
    /// @param fee Fee charged for this verification
    event JobAssigned(
        bytes32 indexed guid,
        uint32 srcEid,
        uint32 dstEid,
        address sender,
        bytes32 receiver,
        bytes32 payloadHash,
        bytes packetHeader,
        uint64 confirmations,
        uint64 nonce,
        bytes options,
        uint256 fee
    );

    /// @notice Emitted when a verification is submitted on destination chain
    /// @param leaf The leaf hash that was verified
    /// @param merkleRoot The Merkle root containing this leaf
    /// @param confirmations Block confirmations for this verification
    event VerificationSubmitted(bytes32 indexed leaf, bytes32 indexed merkleRoot, uint64 confirmations);

    /// @notice Emitted when a Merkle root is cached after signature verification
    /// @param merkleRoot The root that was cached
    /// @param epoch The epoch used for signature verification
    event MerkleRootCached(bytes32 indexed merkleRoot, uint48 epoch);

    /// @notice Emitted when a submitter is added to the whitelist
    /// @param submitter Address of the newly authorized submitter
    event SubmitterAdded(address indexed submitter);

    /// @notice Emitted when a submitter is removed from the whitelist
    /// @param submitter Address of the removed submitter
    event SubmitterRemoved(address indexed submitter);

    /// @notice Emitted during contract initialization
    /// @param settlement Settlement contract address
    /// @param sendUln SendUln302 address
    /// @param receiveUln ReceiveUln302 address
    /// @param localEid Local endpoint ID
    /// @param baseFee Base verification fee
    event Initialized(address settlement, address sendUln, address receiveUln, uint32 localEid, uint256 baseFee);

    /// @notice Emitted when base fee is updated
    /// @param oldFee Previous fee value
    /// @param newFee New fee value
    event BaseFeeUpdated(uint256 oldFee, uint256 newFee);

    /// @notice Emitted when ownership is transferred
    /// @param oldOwner Previous owner address
    /// @param newOwner New owner address
    event OwnershipTransferred(address indexed oldOwner, address indexed newOwner);

    /// @notice Emitted when ownership transfer is initiated
    /// @param oldOwner Current owner address
    /// @param pendingOwner Pending owner address
    event OwnershipTransferStarted(address indexed oldOwner, address indexed pendingOwner);

    /// @notice Emitted when contract is paused
    /// @param account Address that triggered the pause
    event Paused(address account);

    /// @notice Emitted when contract is unpaused
    /// @param account Address that triggered the unpause
    event Unpaused(address account);

    // ============ Constants ============

    /// @notice Maximum signature size to prevent gas griefing
    uint256 public constant MAX_SIGNATURE_SIZE = 8192;

    /// @notice Epoch prefix size in submitter-provided signature payload
    uint256 private constant EPOCH_PREFIX_SIZE = 6;

    /// @notice Minimum proof bytes expected by configured SigVerifierBlsBn254Simple
    uint256 private constant MIN_BLS_PROOF_SIZE = 224;

    /// @notice Minimum signature size: epoch prefix + minimum BLS proof bytes
    uint256 private constant MIN_SIGNATURE_SIZE = EPOCH_PREFIX_SIZE + MIN_BLS_PROOF_SIZE;

    /// @notice Maximum Merkle proof depth (supports trees up to 2^64 leaves)
    uint256 public constant MAX_MERKLE_DEPTH = 64;

    /// @notice Maximum number of leaves accepted in a single batch submission
    uint256 public constant MAX_BATCH_SIZE = 64;

    /// @notice Supported LayerZero packet header version
    uint8 private constant PACKET_VERSION = 1;

    // ============ Immutables ============

    /// @notice Symbiotic settlement contract for quorum verification
    ISettlement public immutable settlement;

    /// @notice Maximum time validity for an epoch's signatures, fixed at deploy time.
    /// Must not exceed the Symbiotic slashing window (misbehaving stake must still be
    /// slashable when a proof is verified). Unused on source-only DVNs (may be zero).
    uint256 public immutable MAX_EPOCH_VALIDITY;

    /// @notice Authorized SendUln302 address (source chain only, address(0) on destination)
    address public immutable sendUln;

    /// @notice ReceiveUln302 address on this chain (destination chain only, address(0) on source)
    address public immutable receiveUln;

    /// @notice Local endpoint ID for this chain
    uint32 public immutable localEid;

    // ============ State Variables ============

    /// @notice Base fee for verification
    uint256 public baseFee;

    /// @notice Owner of the DVN (for admin functions)
    address public owner;

    /// @notice Pending owner that must accept ownership transfer
    address public pendingOwner;

    /// @notice Pause state for emergencies
    bool public paused;

    /// @notice Reentrancy lock
    uint256 private _locked = 1;

    /// @notice Authorized submitters whitelist
    mapping(address => bool) public authorizedSubmitters;

    /// @notice Cached Merkle roots (signature already verified)
    mapping(bytes32 => bool) public verifiedRoots;

    /// @notice Per-leaf duplicate prevention
    mapping(bytes32 => bool) public verifiedLeaves;

    struct BatchProof {
        bytes packetHeader;
        bytes32 payloadHash;
        uint64 confirmations;
        bytes32[] merkleProof;
    }

    // ============ Modifiers ============

    /// @notice Restricts function to contract owner
    modifier onlyOwner() {
        _checkOwner();
        _;
    }

    /// @notice Restricts function to SendUln302 contract
    modifier onlySendUln() {
        if (msg.sender != sendUln) revert OnlySendUln();
        _;
    }

    /// @notice Restricts function to authorized submitters
    modifier onlySubmitter() {
        if (!authorizedSubmitters[msg.sender]) revert UnauthorizedSubmitter(msg.sender);
        _;
    }

    /// @notice Prevents execution when contract is paused
    modifier whenNotPaused() {
        if (paused) revert ContractPaused();
        _;
    }

    /// @notice Prevents reentrancy attacks
    modifier nonReentrant() {
        _lockReentrancyGuard();
        _;
        _locked = 1;
    }

    function _checkOwner() internal view {
        if (msg.sender != owner) revert OnlyOwner();
    }

    function _lockReentrancyGuard() private {
        if (_locked != 1) {
            revert ReentrancyGuardReentrant();
        }
        _locked = 2;
    }

    // ============ Constructor ============

    /// @notice Initialize the DVN contract
    /// @param _settlement Symbiotic Settlement contract address (address(0) on source chain)
    /// @param _sendUln SendUln302 address (source chain) or address(0) (destination chain)
    /// @param _receiveUln ReceiveUln302 address (destination chain) or address(0) (source chain)
    /// @param _localEid This chain's LayerZero endpoint ID
    /// @param _baseFee Base fee for verification jobs
    constructor(
        address _settlement,
        address _sendUln,
        address _receiveUln,
        uint32 _localEid,
        uint256 _baseFee,
        uint256 _maxEpochValidity
    ) {
        if (_localEid == 0) revert InvalidLocalEid();
        if (_sendUln == address(0) && _receiveUln == address(0)) revert InvalidRoleConfiguration();
        if (_receiveUln != address(0) && _settlement == address(0)) revert SettlementRequired();
        if (_receiveUln != address(0) && _maxEpochValidity == 0) revert InvalidEpochValidity();

        settlement = ISettlement(_settlement);
        sendUln = _sendUln;
        receiveUln = _receiveUln;
        localEid = _localEid;
        baseFee = _baseFee;
        MAX_EPOCH_VALIDITY = _maxEpochValidity;
        owner = msg.sender;

        emit Initialized(_settlement, _sendUln, _receiveUln, _localEid, _baseFee);
    }

    // ============ Source Chain Functions ============

    /// @notice Called by LayerZero SendUln302 to assign a verification job
    /// @dev Implements ILayerZeroDVN.assignJob
    /// @dev This function does not accept native fees and reverts if `msg.value` is non-zero.
    /// @param _param Job parameters (dstEid, packetHeader, payloadHash, confirmations, sender)
    /// @param _options Optional parameters (unused in this implementation)
    /// @return fee The fee charged for this job
    function assignJob(
        AssignJobParam calldata _param,
        bytes calldata _options
    ) external payable override onlySendUln whenNotPaused returns (uint256 fee) {
        if (msg.value != 0) revert NoFeeAccepted();
        _validatePacketHeaderFormat(_param.packetHeader);

        uint32 packetDstEid = _packetDstEid(_param.packetHeader);
        if (packetDstEid != _param.dstEid) revert PacketDstEidMismatch(_param.dstEid, packetDstEid);

        if (bytes32(_param.packetHeader[13:45]) != bytes32(uint256(uint160(_param.sender)))) {
            revert SenderMismatch();
        }

        fee = getFee(_param.dstEid, _param.confirmations, _param.sender, _options);

        // Emit event with fields extracted inline to avoid stack too deep
        // Packet header format: version (1) + nonce (8) + srcEid (4) + sender (32) + dstEid (4) + receiver (32) = 81 bytes
        emit JobAssigned(
            keccak256(_param.packetHeader),                     // guid
            uint32(bytes4(_param.packetHeader[9:13])),          // srcEid
            packetDstEid,
            _param.sender,
            bytes32(_param.packetHeader[49:81]),                // receiver
            _param.payloadHash,
            _param.packetHeader,
            _param.confirmations,
            uint64(bytes8(_param.packetHeader[1:9])),           // nonce
            _options,
            fee
        );

        return fee;
    }

    /// @notice Get the fee required for verification
    /// @dev Implements ILayerZeroDVN.getFee
    /// @param /* dstEid */ Destination endpoint ID (unused)
    /// @param /* confirmations */ Block confirmations (unused)
    /// @param /* sender */ Sender address (unused)
    /// @param /* options */ Options (unused)
    /// @return The base fee for verification
    function getFee(
        uint32, /* dstEid */
        uint64, /* confirmations */
        address, /* sender */
        bytes calldata /* options */
    ) public view override returns (uint256) {
        return baseFee;
    }

    // ============ Destination Chain Functions ============

    /// @notice Submit a proof for a single Merkle leaf verification
    /// @dev Called by authorized submitters only. Signature only needed if root not cached.
    /// @param packetHeader The LayerZero packet header (81 bytes)
    /// @param payloadHash Hash of the message payload
    /// @param confirmations Number of block confirmations
    /// @param merkleProof Array of sibling hashes for Merkle proof
    /// @param merkleRoot The Merkle root containing this leaf
    /// @param signature The aggregated BLS quorum signature (only needed if root not cached)
    function submitProof(
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations,
        bytes32[] calldata merkleProof,
        bytes32 merkleRoot,
        bytes calldata signature
    ) external nonReentrant whenNotPaused onlySubmitter {
        if (receiveUln == address(0)) revert ReceiveUlnNotSet();
        _cacheRootIfNeeded(merkleRoot, signature);
        _submitLeafProof(packetHeader, payloadHash, confirmations, merkleProof, merkleRoot);
    }

    /// @notice Submit multiple proofs under a single quorum-signed Merkle root
    /// @dev Signature is only needed when root is not cached. Reverts atomically on any invalid proof.
    /// @param proofs Array of per-leaf proof inputs
    /// @param merkleRoot The Merkle root containing all leaves in `proofs`
    /// @param signature The aggregated BLS quorum signature (only needed if root not cached)
    function submitProofBatch(
        BatchProof[] calldata proofs,
        bytes32 merkleRoot,
        bytes calldata signature
    ) external nonReentrant whenNotPaused onlySubmitter {
        if (receiveUln == address(0)) revert ReceiveUlnNotSet();

        uint256 proofsLength = proofs.length;
        if (proofsLength == 0) revert EmptyBatch();
        if (proofsLength > MAX_BATCH_SIZE) revert BatchTooLarge();

        _cacheRootIfNeeded(merkleRoot, signature);

        for (uint256 i = 0; i < proofsLength; i++) {
            _submitLeafProof(
                proofs[i].packetHeader,
                proofs[i].payloadHash,
                proofs[i].confirmations,
                proofs[i].merkleProof,
                merkleRoot
            );
        }
    }

    /// @notice Cache a Merkle root after quorum signature verification
    /// @dev No-op when root is already cached
    /// @param merkleRoot The Merkle root to cache
    /// @param signature The aggregated BLS quorum signature
    function cacheMerkleRoot(
        bytes32 merkleRoot,
        bytes calldata signature
    ) external nonReentrant whenNotPaused onlySubmitter {
        _cacheRootIfNeeded(merkleRoot, signature);
    }

    // ============ Submitter Management Functions ============

    /// @notice Add a submitter to the authorized whitelist
    /// @param submitter Address to authorize as a submitter
    function addSubmitter(address submitter) external onlyOwner {
        if (authorizedSubmitters[submitter]) revert SubmitterAlreadyAuthorized();
        authorizedSubmitters[submitter] = true;
        emit SubmitterAdded(submitter);
    }

    /// @notice Remove a submitter from the authorized whitelist
    /// @param submitter Address to remove from submitters
    function removeSubmitter(address submitter) external onlyOwner {
        if (!authorizedSubmitters[submitter]) revert SubmitterNotAuthorized();
        authorizedSubmitters[submitter] = false;
        emit SubmitterRemoved(submitter);
    }

    /// @notice Check if an address is an authorized submitter
    /// @param addr Address to check
    /// @return True if the address is an authorized submitter
    function isSubmitter(address addr) external view returns (bool) {
        return authorizedSubmitters[addr];
    }

    // ============ View / Helper Functions ============

    /// @notice Compute the leaf hash for Merkle tree
    /// @param packetHeader The LayerZero packet header
    /// @param payloadHash Hash of the message payload
    /// @param confirmations Number of block confirmations bound to this leaf
    /// @return The computed leaf hash
    function computeLeaf(
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations
    ) public pure returns (bytes32) {
        return keccak256(abi.encodePacked(keccak256(packetHeader), payloadHash, confirmations));
    }

    /// @notice Verify a Merkle proof (can be used off-chain for testing)
    /// @param leaf The leaf to verify
    /// @param proof Array of sibling hashes
    /// @param root The expected Merkle root
    /// @return True if the proof is valid
    function verifyMerkleProof(
        bytes32 leaf,
        bytes32[] calldata proof,
        bytes32 root
    ) external pure returns (bool) {
        return MerkleProof.verifyCalldata(proof, root, leaf);
    }

    /// @notice Check if a leaf has been verified
    /// @param leaf The leaf hash to check
    /// @return True if the leaf has been verified
    function isLeafVerified(bytes32 leaf) external view returns (bool) {
        return verifiedLeaves[leaf];
    }

    /// @notice Check if a root is cached (signature already verified)
    /// @param root The Merkle root to check
    /// @return True if the root is cached
    function isRootVerified(bytes32 root) external view returns (bool) {
        return verifiedRoots[root];
    }

    // ============ Internal Functions ============

    /// @notice Validate packet header format and destination chain
    /// @param packetHeader The LayerZero packet header (81 bytes)
    function _validatePacketHeader(bytes calldata packetHeader) internal view {
        _validatePacketHeaderFormat(packetHeader);

        if (_packetDstEid(packetHeader) != localEid) revert WrongDestinationChain();
    }

    /// @notice Validate packet header format and version
    /// @param packetHeader The LayerZero packet header (81 bytes, version 1)
    function _validatePacketHeaderFormat(bytes calldata packetHeader) internal pure {
        // LayerZero packet header is 81 bytes:
        // version (1) + nonce (8) + srcEid (4) + sender (32) + dstEid (4) + receiver (32)
        if (packetHeader.length != 81) revert InvalidPacketHeader();
        if (uint8(packetHeader[0]) != PACKET_VERSION) revert InvalidPacketVersion();
    }

    /// @notice Extract destination endpoint ID from a LayerZero packet header
    /// @param packetHeader The LayerZero packet header (81 bytes)
    function _packetDstEid(bytes calldata packetHeader) internal pure returns (uint32) {
        // Offset: version (1) + nonce (8) + srcEid (4) + sender (32) = 45
        return uint32(bytes4(packetHeader[45:49]));
    }

    /// @notice Validate epoch is not stale
    /// @param epoch The Symbiotic epoch to validate
    function _validateEpoch(uint48 epoch) internal view {
        // Get epoch capture timestamp
        uint48 epochCaptureTime = settlement.getCaptureTimestampFromValSetHeaderAt(epoch);
        if (epochCaptureTime == 0) revert InvalidEpoch();

        // Check epoch is not expired based on time
        if (block.timestamp > epochCaptureTime + MAX_EPOCH_VALIDITY) revert EpochTooStale();
    }

    /// @notice Cache a root after verifying quorum signature, unless already cached
    /// @param merkleRoot The root to cache
    /// @param signature Signature prefixed with epoch and BLS quorum proof
    function _cacheRootIfNeeded(bytes32 merkleRoot, bytes calldata signature) internal {
        if (verifiedRoots[merkleRoot]) return;
        if (signature.length == 0) revert SignatureRequired();
        if (signature.length < MIN_SIGNATURE_SIZE) revert SignatureTooShort();
        if (signature.length > MAX_SIGNATURE_SIZE) revert SignatureTooLarge();

        // Signature format: epoch (6 bytes) + BLS proof
        uint48 epoch = uint48(bytes6(signature[0:EPOCH_PREFIX_SIZE]));
        bytes calldata blsSignature = signature[EPOCH_PREFIX_SIZE:];

        _validateEpoch(epoch);

        // Message: domain-separated merkle root
        bytes32 messageHash = keccak256(abi.encode(block.chainid, address(this), merkleRoot));
        bytes memory message = abi.encode(messageHash);

        if (
            !settlement.verifyQuorumSigAt(
                message,
                settlement.getRequiredKeyTagFromValSetHeaderAt(epoch),
                settlement.getQuorumThresholdFromValSetHeaderAt(epoch),
                blsSignature,
                epoch,
                new bytes(0)
            )
        ) {
            revert InvalidQuorumSignature();
        }

        verifiedRoots[merkleRoot] = true;
        emit MerkleRootCached(merkleRoot, epoch);
    }

    function _submitLeafProof(
        bytes calldata packetHeader,
        bytes32 payloadHash,
        uint64 confirmations,
        bytes32[] calldata merkleProof,
        bytes32 merkleRoot
    ) internal {
        _validatePacketHeader(packetHeader);

        bytes32 leaf = computeLeaf(packetHeader, payloadHash, confirmations);
        if (verifiedLeaves[leaf]) revert AlreadyVerified();
        if (merkleProof.length > MAX_MERKLE_DEPTH) revert ProofTooLarge();

        if (!MerkleProof.verifyCalldata(merkleProof, merkleRoot, leaf)) {
            revert InvalidMerkleProof();
        }

        verifiedLeaves[leaf] = true;
        IReceiveUlnE2(receiveUln).verify(packetHeader, payloadHash, confirmations);

        emit VerificationSubmitted(leaf, merkleRoot, confirmations);
    }
    // ============ Admin Functions ============

    /// @notice Update base fee
    /// @param _baseFee New base fee
    function setBaseFee(uint256 _baseFee) external onlyOwner {
        if (_baseFee == baseFee) revert BaseFeeUnchanged();
        uint256 oldFee = baseFee;
        baseFee = _baseFee;
        emit BaseFeeUpdated(oldFee, _baseFee);
    }

    /// @notice Withdraw any ETH (e.g., accidentally sent or force-sent)
    /// @param to Address to send ETH to
    function withdraw(address payable to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        (bool success,) = to.call{value: address(this).balance}("");
        if (!success) revert WithdrawFailed();
    }

    /// @notice Initiate ownership transfer (two-step)
    /// @param newOwner Pending owner address
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroOwner();
        if (newOwner == owner) revert OwnerUnchanged();
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Accept ownership transfer
    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert OnlyPendingOwner();
        address oldOwner = owner;
        owner = msg.sender;
        pendingOwner = address(0);
        emit OwnershipTransferred(oldOwner, msg.sender);
    }

    /// @notice Pause the contract
    function pause() external onlyOwner {
        paused = true;
        emit Paused(msg.sender);
    }

    /// @notice Unpause the contract
    function unpause() external onlyOwner {
        paused = false;
        emit Unpaused(msg.sender);
    }

    /// @notice Accept direct ETH transfers to the contract.
    /// @dev `assignJob` rejects `msg.value`; this payable receive exists to accept accidental or force-sent ETH
    /// so the owner can recover it via `withdraw`.
    receive() external payable {}
}
