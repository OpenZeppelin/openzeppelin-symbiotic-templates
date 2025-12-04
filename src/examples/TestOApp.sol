// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {OApp, Origin, MessagingFee, MessagingReceipt} from "@layerzerolabs/lz-evm-oapp-v2/contracts/oapp/OApp.sol";
import {OptionsBuilder} from "@layerzerolabs/lz-evm-oapp-v2/contracts/oapp/libs/OptionsBuilder.sol";

/// @title TestOApp
/// @notice Simple OApp for testing cross-chain messaging with SymbioticLayerZeroDVN
/// @dev Deploy on both source and destination chains, then configure peers
contract TestOApp is OApp {
    using OptionsBuilder for bytes;

    /// @notice Emitted when a message is sent
    event MessageSent(uint32 indexed dstEid, bytes32 indexed receiver, bytes message);

    /// @notice Emitted when a message is received
    event MessageReceived(uint32 indexed srcEid, bytes32 indexed sender, bytes message);

    /// @notice Counter for received messages
    uint256 public messagesReceived;

    /// @notice Last received message
    bytes public lastMessage;

    /// @notice Last sender address
    bytes32 public lastSender;

    /// @notice Last source chain endpoint ID
    uint32 public lastSrcEid;

    constructor(address _endpoint, address _delegate) OApp(_endpoint, _delegate) {}

    /// @notice Send a message to another chain
    /// @param _dstEid Destination endpoint ID
    /// @param _message Message to send
    /// @param _options LayerZero execution options
    function send(
        uint32 _dstEid,
        bytes calldata _message,
        bytes calldata _options
    ) external payable returns (MessagingReceipt memory receipt) {
        bytes32 receiver = peers[_dstEid];
        require(receiver != bytes32(0), "Peer not set");

        receipt = _lzSend(_dstEid, _message, _options, MessagingFee(msg.value, 0), payable(msg.sender));

        emit MessageSent(_dstEid, receiver, _message);
    }

    /// @notice Send a simple "ping" message
    /// @param _dstEid Destination endpoint ID
    function ping(uint32 _dstEid) external payable returns (MessagingReceipt memory receipt) {
        bytes memory message = abi.encode("ping", block.timestamp, block.number);
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(200000, 0);

        receipt = _lzSend(_dstEid, message, options, MessagingFee(msg.value, 0), payable(msg.sender));

        emit MessageSent(_dstEid, peers[_dstEid], message);
    }

    /// @notice Quote the fee for sending a message
    /// @param _dstEid Destination endpoint ID
    /// @param _message Message to send
    /// @param _options LayerZero execution options
    /// @return fee The messaging fee
    function quote(
        uint32 _dstEid,
        bytes calldata _message,
        bytes calldata _options
    ) external view returns (MessagingFee memory fee) {
        return _quote(_dstEid, _message, _options, false);
    }

    /// @notice Quote the fee for a ping message
    /// @param _dstEid Destination endpoint ID
    /// @return fee The messaging fee
    function quotePing(uint32 _dstEid) external view returns (MessagingFee memory fee) {
        bytes memory message = abi.encode("ping", block.timestamp, block.number);
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(200000, 0);
        return _quote(_dstEid, message, options, false);
    }

    /// @notice Internal receive handler
    function _lzReceive(
        Origin calldata _origin,
        bytes32 /*_guid*/,
        bytes calldata _message,
        address /*_executor*/,
        bytes calldata /*_extraData*/
    ) internal override {
        messagesReceived++;
        lastMessage = _message;
        lastSender = _origin.sender;
        lastSrcEid = _origin.srcEid;

        emit MessageReceived(_origin.srcEid, _origin.sender, _message);
    }

    /// @notice Check if a peer is set for a destination
    /// @param _eid Endpoint ID to check
    /// @return Whether the peer is set
    function hasPeer(uint32 _eid) external view returns (bool) {
        return peers[_eid] != bytes32(0);
    }

    /// @notice Mock receive for testing - allows simulating message receipt
    /// @dev Only callable by the endpoint (for testing with mock endpoints)
    function mockReceive(
        uint32 _srcEid,
        bytes32 _sender,
        uint64 _nonce,
        bytes calldata _message
    ) external {
        // In production this would be protected, but for testing we allow it
        messagesReceived++;
        lastMessage = _message;
        lastSender = _sender;
        lastSrcEid = _srcEid;

        emit MessageReceived(_srcEid, _sender, _message);
    }

    /// @notice Receive native currency
    receive() external payable {}
}
