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
    error InvalidQuorumSignature();
    error InvalidVerifierResults();
    error InvalidCCVVersion(bytes4 got);
    error InvalidSenderEncoding(uint256 length);
    error InvalidSenderEncodingUpperBytes(bytes32 sender);

    uint256 public constant VERSION_BYTES = 4;
    uint256 public constant EPOCH_BYTES = 6;
    uint256 public constant MIN_VERIFIER_RESULTS_BYTES = VERSION_BYTES + EPOCH_BYTES + 1;
    uint256 public constant MAX_EPOCH_VALIDITY = 7200;

    string public constant override typeAndVersion = "SymbioticVerifier 1.0.0";

    ISettlement public immutable settlement;

    constructor(
        address settlementAddress,
        string[] memory storageLocations,
        address rmn,
        bytes4 verifierVersionTag
    ) BaseVerifier(storageLocations, rmn, verifierVersionTag) {
        if (settlementAddress == address(0)) {
            revert ZeroAddressNotAllowed();
        }
        settlement = ISettlement(settlementAddress);
    }

    /// @inheritdoc ICrossChainVerifierV1
    function verifyMessage(
        MessageV1Codec.MessageV1 memory message,
        bytes32 messageId,
        bytes memory verifierResults
    ) external view override {
        _assertNotCursedByRMN(message.sourceChainSelector);
        _onlyOffRamp(message.sourceChainSelector);

        if (verifierResults.length < MIN_VERIFIER_RESULTS_BYTES) {
            revert InvalidVerifierResults();
        }

        bytes4 verifierVersion = _extractVersion(verifierResults);
        if (verifierVersion != versionTag()) {
            revert InvalidCCVVersion(verifierVersion);
        }

        uint48 epoch = _extractEpoch(verifierResults);
        bytes memory blsSignature = _extractSignature(verifierResults);
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

    function _extractVersion(bytes memory verifierResults) internal pure returns (bytes4 version) {
        uint32 raw = (uint32(uint8(verifierResults[0])) << 24)
            | (uint32(uint8(verifierResults[1])) << 16)
            | (uint32(uint8(verifierResults[2])) << 8)
            | uint32(uint8(verifierResults[3]));
        return bytes4(raw);
    }

    function _extractEpoch(bytes memory verifierResults) internal pure returns (uint48 epoch) {
        return
            (uint48(uint8(verifierResults[VERSION_BYTES])) << 40)
            | (uint48(uint8(verifierResults[VERSION_BYTES + 1])) << 32)
            | (uint48(uint8(verifierResults[VERSION_BYTES + 2])) << 24)
            | (uint48(uint8(verifierResults[VERSION_BYTES + 3])) << 16)
            | (uint48(uint8(verifierResults[VERSION_BYTES + 4])) << 8)
            | uint48(uint8(verifierResults[VERSION_BYTES + 5]));
    }

    function _extractSignature(bytes memory verifierResults) internal pure returns (bytes memory signature) {
        uint256 signatureOffset = VERSION_BYTES + EPOCH_BYTES;
        uint256 signatureLength = verifierResults.length - signatureOffset;
        signature = new bytes(signatureLength);
        for (uint256 i = 0; i < signatureLength; ++i) {
            signature[i] = verifierResults[signatureOffset + i];
        }
    }

    function _validateEpoch(uint48 epoch) internal view {
        uint48 captureTime = settlement.getCaptureTimestampFromValSetHeaderAt(epoch);
        if (captureTime == 0) {
            revert InvalidEpoch();
        }
        if (block.timestamp > captureTime + MAX_EPOCH_VALIDITY) {
            revert EpochTooStale();
        }
    }
}
