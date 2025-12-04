// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {OApp, MessagingFee, Origin, MessagingReceipt} from "@layerzerolabs/lz-evm-oapp-v2/contracts/oapp/OApp.sol";
import {OptionsBuilder} from "@layerzerolabs/lz-evm-oapp-v2/contracts/oapp/libs/OptionsBuilder.sol";

/// @title TestOApp
/// @notice Simple OApp for testing cross-chain messaging via LayerZero
/// @dev Used for E2E testing of Symbiotic LayerZero DVN
contract TestOApp is OApp {
    using OptionsBuilder for bytes;

    event MessageSent(uint32 indexed dstEid, bytes32 guid, bytes message);
    event MessageReceived(uint32 indexed srcEid, bytes32 sender, bytes message);

    /// @dev Gas limit for lzReceive execution on destination
    uint128 public constant DEST_GAS_LIMIT = 200_000;

    constructor(address _endpoint, address _delegate) OApp(_endpoint, _delegate) {}

    /// @notice Send a cross-chain message
    /// @param _dstEid Destination endpoint ID
    /// @param _message Message payload
    /// @return guid The unique message identifier
    function send(uint32 _dstEid, bytes calldata _message) external payable returns (bytes32 guid) {
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(DEST_GAS_LIMIT, 0);

        MessagingFee memory fee = _quote(_dstEid, _message, options, false);
        require(msg.value >= fee.nativeFee, "Insufficient fee");

        MessagingReceipt memory receipt =
            _lzSend(_dstEid, _message, options, MessagingFee(msg.value, 0), payable(msg.sender));

        emit MessageSent(_dstEid, receipt.guid, _message);
        return receipt.guid;
    }

    /// @notice Quote the fee for sending a message
    /// @param _dstEid Destination endpoint ID
    /// @param _message Message payload
    /// @return nativeFee The native token fee required
    function quote(uint32 _dstEid, bytes calldata _message) external view returns (uint256 nativeFee) {
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(DEST_GAS_LIMIT, 0);
        return _quote(_dstEid, _message, options, false).nativeFee;
    }

    /// @dev Internal function to handle received messages
    function _lzReceive(
        Origin calldata _origin,
        bytes32 _guid,
        bytes calldata _message,
        address, /*_executor*/
        bytes calldata /*_extraData*/
    ) internal override {
        emit MessageReceived(_origin.srcEid, _origin.sender, _message);
    }
}
