// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Ownable2StepMsgSender} from
    "@chainlink/contracts/src/v0.8/shared/access/Ownable2StepMsgSender.sol";
import {ICrossChainVerifierV1} from
    "@chainlink/contracts-ccip/contracts/interfaces/ICrossChainVerifierV1.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {ISettlement} from "../interfaces/ISettlement.sol";

/// @title SymbioticVerifier
/// @notice CCIP verifier implementation secured by Symbiotic settlement verification.
contract SymbioticVerifier is Ownable2StepMsgSender, ICrossChainVerifierV1, BaseVerifier {
    error InvalidEpoch();
    error EpochTooStale();
    error EpochBelowMinimum(uint48 epoch, uint48 minAcceptedEpoch);
    error InvalidQuorumSignature();
    error InvalidVerifierResults();
    error InvalidCCVVersion(bytes4 got);
    error InvalidSenderEncoding(uint256 length);
    error InvalidSenderEncodingUpperBytes(bytes32 sender);
    error InvalidEpochValidity(uint256 epochValidity);

    event EpochValiditySet(uint256 epochValidity);

    event MinAcceptedEpochSet(uint48 minAcceptedEpoch);

    uint256 public constant VERSION_BYTES = 4;
    uint256 public constant EPOCH_BYTES = 6;
    uint256 public constant MIN_VERIFIER_RESULTS_BYTES = VERSION_BYTES + EPOCH_BYTES + 1;

    string public constant override typeAndVersion = "SymbioticVerifier 1.0.0";

    ISettlement public immutable settlement;

    /// @dev Ceiling for the owner-settable epoch validity window, fixed at deploy time.
    /// The window caps how old the attesting validator set may be at verification time
    /// (a freshness/rotation bound). For forks that wire up slashing, the ceiling must
    /// not exceed the Symbiotic slashing window, so that misbehaving stake is still
    /// slashable when a proof is verified — this template ships with no slashing path,
    /// so as shipped the bound is operational, not economic. Deploy scripts derive it
    /// from the deployment's `slashingWindowSeconds`. NOTE: this also caps incident
    /// recovery — messages attested more than `maxEpochValidity` before recovery
    /// completes cannot be verified without redeploying the verifier.
    uint256 public immutable maxEpochValidity;

    /// @dev Maximum age of an epoch's valset capture accepted by `verifyMessage`.
    /// Owner may raise it temporarily (never above `maxEpochValidity`) to recover
    /// messages attested before an infra outage, then restore the usual value —
    /// deploy below the ceiling to keep that headroom available.
    uint256 private s_epochValidity;

    /// @dev Floor on the attesting epoch accepted by `verifyMessage`. The epoch in
    /// `verifierResults` is prover-supplied and selects the validator set, key tag,
    /// quorum threshold, AND the sig-verifier implementation for that epoch — so any
    /// still-fresh older epoch remains acceptable by default. Raising this floor is
    /// the emergency lever that immediately revokes older epochs (e.g. after a quorum
    /// threshold raise, key-tag rotation, or sig-verifier replacement) without waiting
    /// for them to age out of the validity window.
    uint48 private s_minAcceptedEpoch;

    constructor(
        address settlementAddress,
        string[] memory storageLocations,
        address rmn,
        bytes4 verifierVersionTag,
        uint256 maxEpochValidity_,
        uint256 initialEpochValidity
    ) BaseVerifier(storageLocations, rmn, verifierVersionTag) {
        if (settlementAddress == address(0)) {
            revert ZeroAddressNotAllowed();
        }
        if (
            maxEpochValidity_ == 0 || initialEpochValidity == 0
                || initialEpochValidity > maxEpochValidity_
        ) {
            revert InvalidEpochValidity(initialEpochValidity);
        }
        settlement = ISettlement(settlementAddress);
        maxEpochValidity = maxEpochValidity_;
        s_epochValidity = initialEpochValidity;
        emit EpochValiditySet(initialEpochValidity);
    }

    /// @inheritdoc ICrossChainVerifierV1
    /// @dev `verifierResults` wire format (consensus-critical, no length framing):
    /// `versionTag (4 bytes) ‖ epoch (6 bytes, uint48 big-endian) ‖ BLS aggregate
    /// proof (variable, consumed whole by Settlement)`. The operator encodes the
    /// identical layout in `encode_ccv_data` (operator/src/provider/chainlink_ccv.rs);
    /// the two must agree byte-for-byte. This packed layout deliberately deviates
    /// from Chainlink's length-prefixed CommitteeVerifier format — any layout change
    /// requires a new versionTag.
    function verifyMessage(
        MessageV1Codec.MessageV1 memory message,
        bytes32 messageId,
        bytes calldata verifierResults
    ) external view override {
        _assertNotCursedByRMN(message.sourceChainSelector);
        _onlyOffRamp(message.sourceChainSelector);

        if (verifierResults.length < MIN_VERIFIER_RESULTS_BYTES) {
            revert InvalidVerifierResults();
        }

        bytes4 verifierVersion = bytes4(verifierResults[:VERSION_BYTES]);
        if (verifierVersion != versionTag()) {
            revert InvalidCCVVersion(verifierVersion);
        }

        uint48 epoch = uint48(bytes6(verifierResults[VERSION_BYTES:VERSION_BYTES + EPOCH_BYTES]));
        // Calldata slice instead of a memory copy: the proof grows with validator count and a
        // byte-wise copy dominated verifyMessage gas (~85% at 100 validators).
        bytes calldata blsSignature = verifierResults[VERSION_BYTES + EPOCH_BYTES:];
        _validateEpoch(epoch);

        bytes32 signedDigest = keccak256(bytes.concat(versionTag(), messageId));
        bool validSignature = settlement.verifyQuorumSigAt(
            abi.encode(signedDigest),
            settlement.getRequiredKeyTagFromValSetHeaderAt(epoch),
            settlement.getQuorumThresholdFromValSetHeaderAt(epoch),
            blsSignature,
            epoch,
            new bytes(0)
        );

        if (!validSignature) {
            revert InvalidQuorumSignature();
        }
    }

    /// @inheritdoc ICrossChainVerifierV1
    function forwardToVerifier(
        MessageV1Codec.MessageV1 calldata message,
        bytes32,
        address,
        uint256,
        bytes calldata
    ) external view override returns (bytes memory verifierData) {
        _assertNotCursedByRMN(message.destChainSelector);
        address sender = _decodeSender(message.sender);
        _assertSenderIsAllowed(message.destChainSelector, sender);
        return abi.encodePacked(versionTag());
    }

    function applyRemoteChainConfigUpdates(RemoteChainConfigArgs[] calldata configArgs) external onlyOwner {
        _applyRemoteChainConfigUpdates(configArgs);
    }

    function applyAllowlistUpdates(AllowlistConfigArgs[] calldata configArgs) external onlyOwner {
        _applyAllowlistUpdates(configArgs);
    }

    function setAllowedFinalityConfig(bytes4 allowedFinality) external onlyOwner {
        _setAllowedFinalityConfig(allowedFinality);
    }

    function updateStorageLocations(string[] memory storageLocations) external onlyOwner {
        _setStorageLocations(storageLocations);
    }

    /// @notice Sets the maximum accepted age of the attesting epoch's valset capture.
    /// @param epochValidity New validity window in seconds; must be non-zero and at
    /// most `maxEpochValidity`.
    function setEpochValidity(uint256 epochValidity) external onlyOwner {
        if (epochValidity == 0 || epochValidity > maxEpochValidity) {
            revert InvalidEpochValidity(epochValidity);
        }
        s_epochValidity = epochValidity;
        emit EpochValiditySet(epochValidity);
    }

    /// @notice Returns the current epoch validity window in seconds.
    function getEpochValidity() external view returns (uint256) {
        return s_epochValidity;
    }

    /// @notice Sets the minimum attesting epoch accepted by `verifyMessage`.
    /// Raise it to immediately revoke still-fresh older epochs after a security
    /// parameter change (quorum threshold raise, key-tag rotation, sig-verifier
    /// replacement). Setting it above the latest committed epoch pauses
    /// verification until the next header is committed.
    function setMinAcceptedEpoch(uint48 minAcceptedEpoch) external onlyOwner {
        s_minAcceptedEpoch = minAcceptedEpoch;
        emit MinAcceptedEpochSet(minAcceptedEpoch);
    }

    /// @notice Returns the minimum attesting epoch accepted by `verifyMessage`.
    function getMinAcceptedEpoch() external view returns (uint48) {
        return s_minAcceptedEpoch;
    }

    function _decodeSender(bytes memory encodedSender) internal pure returns (address) {
        if (encodedSender.length == 32) {
            bytes32 sender = abi.decode(encodedSender, (bytes32));
            if (uint256(sender) >> 160 != 0) {
                revert InvalidSenderEncodingUpperBytes(sender);
            }
            return address(uint160(uint256(sender)));
        }
        if (encodedSender.length == 20) {
            return address(bytes20(encodedSender));
        }
        revert InvalidSenderEncoding(encodedSender.length);
    }

    function _validateEpoch(uint48 epoch) internal view {
        if (epoch < s_minAcceptedEpoch) {
            revert EpochBelowMinimum(epoch, s_minAcceptedEpoch);
        }
        uint48 captureTime = settlement.getCaptureTimestampFromValSetHeaderAt(epoch);
        if (captureTime == 0) {
            revert InvalidEpoch();
        }
        if (block.timestamp > captureTime + s_epochValidity) {
            revert EpochTooStale();
        }
    }
}
