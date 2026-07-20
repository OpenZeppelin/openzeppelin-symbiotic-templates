// SPDX-License-Identifier: MIT
pragma solidity >=0.8.0;

/// @dev should be implemented by the ReceiveUln302 contract and future ReceiveUln contracts on EndpointV2
interface IReceiveUlnE2 {
    /// @notice for each dvn to verify the payload
    /// @dev this function signature 0x0223536e
    /// @param _packetHeader LayerZero packet header (version + nonce + path)
    /// @param _payloadHash Hash of guid + message payload
    /// @param _confirmations Block confirmation delay before verification
    function verify(bytes calldata _packetHeader, bytes32 _payloadHash, uint64 _confirmations) external;

    /// @notice verify the payload at endpoint, will check if all DVNs verified
    /// @param _packetHeader LayerZero packet header identifying the packet
    /// @param _payloadHash Hash of guid + message payload
    function commitVerification(bytes calldata _packetHeader, bytes32 _payloadHash) external;
}
