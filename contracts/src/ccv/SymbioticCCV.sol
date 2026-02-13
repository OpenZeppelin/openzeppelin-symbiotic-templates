// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {ISettlement} from "../interfaces/ISettlement.sol";
import {ICrossChainVerifierResolver} from "./interfaces/ICrossChainVerifierResolver.sol";
import {ICrossChainVerifierV1} from "./interfaces/ICrossChainVerifierV1.sol";
import {Client} from "./libraries/Client.sol";
import {MessageV1Codec} from "./libraries/MessageV1Codec.sol";

/// @title SymbioticCCV
/// @notice Minimal CCIP CCV implementation secured by Symbiotic settlement verification.
/// @dev Implements both verifier and resolver interfaces so it can be used directly as a CCV address in OnRamp/OffRamp.
contract SymbioticCCV is Ownable, ICrossChainVerifierV1, ICrossChainVerifierResolver {
    error ZeroAddressNotAllowed();
    error InvalidRemoteChainConfig(uint64 remoteChainSelector);
    error RemoteChainNotSupported(uint64 remoteChainSelector);
    error CallerIsNotConfiguredOnRamp(address caller);
    error CallerIsNotConfiguredOffRamp(address caller);
    error SenderNotAllowed(address sender);
    error InvalidSenderEncoding(uint256 length);
    error InvalidSenderEncodingUpperBytes(bytes32 sender);
    error InvalidVerifierResults();
    error InvalidCCVVersion(bytes4 got);
    error InvalidEpoch();
    error EpochTooStale();
    error InvalidQuorumSignature();

    event RemoteChainConfigSet(
        uint64 indexed remoteChainSelector,
        address onRamp,
        address offRamp,
        bool allowlistEnabled,
        uint16 feeUSDCents,
        uint32 gasForVerification,
        uint32 payloadSizeBytes
    );
    event AllowListSendersAdded(uint64 indexed remoteChainSelector, address[] senders);
    event AllowListSendersRemoved(uint64 indexed remoteChainSelector, address[] senders);
    event StorageLocationsUpdated(string[] oldLocations, string[] newLocations);

    struct RemoteChainConfig {
        address onRamp;
        address offRamp;
        uint16 feeUSDCents;
        uint32 gasForVerification;
        uint32 payloadSizeBytes;
        bool allowlistEnabled;
    }

    struct RemoteChainConfigArgs {
        uint64 remoteChainSelector;
        address onRamp;
        address offRamp;
        bool allowlistEnabled;
        uint16 feeUSDCents;
        uint32 gasForVerification;
        uint32 payloadSizeBytes;
    }

    struct AllowlistConfigArgs {
        uint64 remoteChainSelector;
        bool allowlistEnabled;
        address[] addedAllowlistedSenders;
        address[] removedAllowlistedSenders;
    }

    /// @dev Prefix tag used by resolver and signed payload construction.
    /// keccak256("SymbioticCCV 1.0.0")[:4]
    bytes4 public constant VERSION_TAG_V1_0_0 = 0x1a75bd93;
    uint256 public constant VERSION_BYTES = 4;
    uint256 public constant EPOCH_BYTES = 6;
    uint256 public constant MIN_VERIFIER_RESULTS_BYTES = VERSION_BYTES + EPOCH_BYTES + 1;

    /// @notice Maximum validity window for a settlement epoch.
    uint256 public constant MAX_EPOCH_VALIDITY = 7200;

    string public constant typeAndVersion = "SymbioticCCV 1.0.0";

    ISettlement public immutable settlement;

    mapping(uint64 remoteChainSelector => RemoteChainConfig config) private s_remoteChainConfigs;
    mapping(uint64 remoteChainSelector => mapping(address sender => bool isAllowed)) private s_allowedSenders;

    string[] private s_storageLocations;

    constructor(address settlementAddress, string[] memory storageLocations) Ownable(msg.sender) {
        if (settlementAddress == address(0)) {
            revert ZeroAddressNotAllowed();
        }

        settlement = ISettlement(settlementAddress);
        _setStorageLocations(storageLocations);
    }

    /// @inheritdoc ICrossChainVerifierV1
    function verifyMessage(
        MessageV1Codec.MessageV1 memory message,
        bytes32 messageId,
        bytes memory verifierResults
    ) external view override {
        RemoteChainConfig storage cfg = s_remoteChainConfigs[message.sourceChainSelector];
        if (cfg.offRamp == address(0)) {
            revert RemoteChainNotSupported(message.sourceChainSelector);
        }
        if (msg.sender != cfg.offRamp) {
            revert CallerIsNotConfiguredOffRamp(msg.sender);
        }

        if (verifierResults.length < MIN_VERIFIER_RESULTS_BYTES) {
            revert InvalidVerifierResults();
        }

        bytes4 version = _extractVersion(verifierResults);
        if (version != VERSION_TAG_V1_0_0) {
            revert InvalidCCVVersion(version);
        }

        uint48 epoch = _extractEpoch(verifierResults);
        bytes memory blsSignature = _extractSignature(verifierResults);
        if (blsSignature.length == 0) {
            revert InvalidVerifierResults();
        }

        _validateEpoch(epoch);

        // Bind verifier version + message ID so resolver version cannot be swapped post-signature.
        bytes32 signedDigest = keccak256(bytes.concat(version, messageId));

        bool ok = settlement.verifyQuorumSigAt(
            abi.encode(signedDigest),
            settlement.getRequiredKeyTagFromValSetHeaderAt(epoch),
            settlement.getQuorumThresholdFromValSetHeaderAt(epoch),
            blsSignature,
            epoch,
            new bytes(0)
        );

        if (!ok) {
            revert InvalidQuorumSignature();
        }
    }

    /// @inheritdoc ICrossChainVerifierV1
    function getFee(
        uint64 destChainSelector,
        Client.EVM2AnyMessage memory,
        bytes memory,
        uint16
    ) external view override returns (uint16 feeUSDCents, uint32 gasForVerification, uint32 payloadSizeBytes) {
        RemoteChainConfig storage cfg = s_remoteChainConfigs[destChainSelector];
        if (cfg.onRamp == address(0)) {
            revert RemoteChainNotSupported(destChainSelector);
        }

        return (cfg.feeUSDCents, cfg.gasForVerification, cfg.payloadSizeBytes);
    }

    /// @inheritdoc ICrossChainVerifierV1
    function forwardToVerifier(
        MessageV1Codec.MessageV1 calldata message,
        bytes32,
        address,
        uint256,
        bytes calldata
    ) external view override returns (bytes memory verifierData) {
        RemoteChainConfig storage cfg = s_remoteChainConfigs[message.destChainSelector];
        if (cfg.onRamp == address(0)) {
            revert RemoteChainNotSupported(message.destChainSelector);
        }
        if (msg.sender != cfg.onRamp) {
            revert CallerIsNotConfiguredOnRamp(msg.sender);
        }

        address sender = _decodeSender(message.sender);

        if (cfg.allowlistEnabled && !s_allowedSenders[message.destChainSelector][sender]) {
            revert SenderNotAllowed(sender);
        }

        return abi.encodePacked(VERSION_TAG_V1_0_0);
    }

    /// @inheritdoc ICrossChainVerifierV1
    function getStorageLocations() external view override returns (string[] memory) {
        return s_storageLocations;
    }

    /// @inheritdoc ICrossChainVerifierResolver
    function getInboundImplementation(bytes calldata verifierResults) external view override returns (address) {
        if (verifierResults.length < VERSION_BYTES) {
            return address(0);
        }

        bytes4 version = bytes4(verifierResults[:VERSION_BYTES]);
        return version == VERSION_TAG_V1_0_0 ? address(this) : address(0);
    }

    /// @inheritdoc ICrossChainVerifierResolver
    function getOutboundImplementation(uint64 destChainSelector, bytes memory) external view override returns (address) {
        return s_remoteChainConfigs[destChainSelector].onRamp != address(0) ? address(this) : address(0);
    }

    function applyRemoteChainConfigUpdates(RemoteChainConfigArgs[] calldata configArgs) external onlyOwner {
        for (uint256 i = 0; i < configArgs.length; ++i) {
            RemoteChainConfigArgs memory arg = configArgs[i];

            if (arg.remoteChainSelector == 0 || arg.onRamp == address(0) || arg.offRamp == address(0)) {
                revert InvalidRemoteChainConfig(arg.remoteChainSelector);
            }
            if (arg.gasForVerification == 0) {
                revert InvalidRemoteChainConfig(arg.remoteChainSelector);
            }

            s_remoteChainConfigs[arg.remoteChainSelector] = RemoteChainConfig({
                onRamp: arg.onRamp,
                offRamp: arg.offRamp,
                feeUSDCents: arg.feeUSDCents,
                gasForVerification: arg.gasForVerification,
                payloadSizeBytes: arg.payloadSizeBytes,
                allowlistEnabled: arg.allowlistEnabled
            });

            emit RemoteChainConfigSet(
                arg.remoteChainSelector,
                arg.onRamp,
                arg.offRamp,
                arg.allowlistEnabled,
                arg.feeUSDCents,
                arg.gasForVerification,
                arg.payloadSizeBytes
            );
        }
    }

    function applyAllowlistUpdates(AllowlistConfigArgs[] calldata allowlistConfigArgsItems) external onlyOwner {
        for (uint256 i = 0; i < allowlistConfigArgsItems.length; ++i) {
            AllowlistConfigArgs memory arg = allowlistConfigArgsItems[i];
            if (s_remoteChainConfigs[arg.remoteChainSelector].onRamp == address(0)) {
                revert RemoteChainNotSupported(arg.remoteChainSelector);
            }

            s_remoteChainConfigs[arg.remoteChainSelector].allowlistEnabled = arg.allowlistEnabled;

            for (uint256 j = 0; j < arg.addedAllowlistedSenders.length; ++j) {
                s_allowedSenders[arg.remoteChainSelector][arg.addedAllowlistedSenders[j]] = true;
            }
            for (uint256 j = 0; j < arg.removedAllowlistedSenders.length; ++j) {
                s_allowedSenders[arg.remoteChainSelector][arg.removedAllowlistedSenders[j]] = false;
            }

            if (arg.addedAllowlistedSenders.length > 0) {
                emit AllowListSendersAdded(arg.remoteChainSelector, arg.addedAllowlistedSenders);
            }
            if (arg.removedAllowlistedSenders.length > 0) {
                emit AllowListSendersRemoved(arg.remoteChainSelector, arg.removedAllowlistedSenders);
            }
        }
    }

    function updateStorageLocations(string[] memory storageLocations) external onlyOwner {
        _setStorageLocations(storageLocations);
    }

    function isSenderAllowlisted(uint64 remoteChainSelector, address sender) external view returns (bool) {
        return s_allowedSenders[remoteChainSelector][sender];
    }

    function getRemoteChainConfig(
        uint64 remoteChainSelector
    ) external view returns (RemoteChainConfig memory config) {
        return s_remoteChainConfigs[remoteChainSelector];
    }

    function supportsInterface(bytes4 interfaceId) external pure override returns (bool) {
        return interfaceId == type(ICrossChainVerifierV1).interfaceId
            || interfaceId == type(ICrossChainVerifierResolver).interfaceId
            || interfaceId == type(IERC165).interfaceId;
    }

    function _setStorageLocations(string[] memory storageLocations) internal {
        string[] memory oldLocations = s_storageLocations;

        delete s_storageLocations;
        for (uint256 i = 0; i < storageLocations.length; ++i) {
            s_storageLocations.push(storageLocations[i]);
        }

        emit StorageLocationsUpdated(oldLocations, storageLocations);
    }

    function _decodeSender(bytes memory sender) internal pure returns (address) {
        if (sender.length == 32) {
            bytes32 encoded = abi.decode(sender, (bytes32));
            if (uint256(encoded) >> 160 != 0) {
                revert InvalidSenderEncodingUpperBytes(encoded);
            }
            return address(uint160(uint256(encoded)));
        }
        if (sender.length == 20) {
            return address(bytes20(sender));
        }
        revert InvalidSenderEncoding(sender.length);
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

    function _extractSignature(bytes memory verifierResults) internal pure returns (bytes memory blsSignature) {
        uint256 sigOffset = VERSION_BYTES + EPOCH_BYTES;
        uint256 sigLength = verifierResults.length - sigOffset;
        blsSignature = new bytes(sigLength);

        for (uint256 i = 0; i < sigLength; ++i) {
            blsSignature[i] = verifierResults[sigOffset + i];
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
