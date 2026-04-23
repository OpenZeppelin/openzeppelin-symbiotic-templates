// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { OApp, MessagingFee, Origin } from "@layerzerolabs/oapp-evm/contracts/oapp/OApp.sol";
import { MessagingReceipt } from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { OptionsBuilder } from "@layerzerolabs/oapp-evm/contracts/oapp/libs/OptionsBuilder.sol";

/// @title ExampleOApp
/// @notice Starter OApp demonstrating LayerZero cross-chain messaging with the Symbiotic DVN
/// @dev This contract is intended as a base application for template users.
///      Replace or customize it as needed for production deployments.
///
/// The ExampleOApp demonstrates the basic pattern for:
/// - Sending cross-chain messages via LayerZero
/// - Receiving and processing messages from other chains
/// - Quoting fees for cross-chain transactions
/// - Using the OptionsBuilder for execution options
///
/// Message Flow:
/// 1. User calls send() on source chain
/// 2. LayerZero endpoint routes to SendUln302
/// 3. SendUln302 assigns verification job to DVN
/// 4. Symbiotic operators verify the message via BLS signatures
/// 5. Relayer submits proof to destination DVN
/// 6. DVN calls ReceiveUln302.verify()
/// 7. Executor calls lzReceive() on destination OApp
contract ExampleOApp is OApp {
    using OptionsBuilder for bytes;

    /// @notice The last message received from another chain
    string public lastMessage;

    /// @notice The source chain endpoint ID of the last received message
    uint32 public lastSrcEid;

    /// @notice The sender address (as bytes32) of the last received message
    bytes32 public lastSender;

    /// @notice Counter for messages sent from this contract
    uint256 public messagesSent;

    /// @notice Counter for messages received by this contract
    uint256 public messagesReceived;

    /// @notice Emitted when a message is sent to another chain
    /// @param dstEid The destination endpoint ID
    /// @param message The message content
    /// @param guid The unique message identifier
    /// @param nonce The message nonce
    event MessageSent(uint32 indexed dstEid, string message, bytes32 guid, uint64 nonce);

    /// @notice Emitted when a message is received from another chain
    /// @param srcEid The source endpoint ID
    /// @param sender The sender address (as bytes32)
    /// @param message The message content
    /// @param guid The unique message identifier
    event MessageReceived(uint32 indexed srcEid, bytes32 sender, string message, bytes32 guid);

    /// @notice Creates a new ExampleOApp instance
    /// @param _endpoint The address of the local LayerZero endpoint
    /// @param _delegate The delegate address capable of making OApp configurations
    /// @dev The delegate is typically set to the owner/deployer of the contract
    constructor(address _endpoint, address _delegate) OApp(_endpoint, _delegate) Ownable(_delegate) { }

    /// @notice Sends a message to a destination chain
    /// @param _dstEid The endpoint ID of the destination chain
    /// @param _message The message string to send
    /// @param _options Execution options (gas limits, etc.)
    /// @return receipt The messaging receipt containing guid and nonce
    /// @dev Requires msg.value to cover the messaging fee
    ///
    /// Example usage:
    /// ```solidity
    /// bytes memory options = OptionsBuilder.newOptions()
    ///     .addExecutorLzReceiveOption(200000, 0);
    /// uint256 fee = exampleOApp.quote(dstEid, "Hello", options, false).nativeFee;
    /// exampleOApp.send{value: fee}(dstEid, "Hello", options);
    /// ```
    function send(
        uint32 _dstEid,
        string calldata _message,
        bytes calldata _options
    )
        external
        payable
        returns (MessagingReceipt memory receipt)
    {
        bytes memory payload = abi.encode(_message);

        receipt = _lzSend(_dstEid, payload, _options, MessagingFee(msg.value, 0), payable(msg.sender));

        messagesSent++;

        emit MessageSent(_dstEid, _message, receipt.guid, receipt.nonce);
    }

    /// @notice Quotes the fee required to send a message
    /// @param _dstEid The destination endpoint ID
    /// @param _message The message to send
    /// @param _options Execution options
    /// @param _payInLzToken Whether to pay the fee in LZ tokens (false = native token)
    /// @return fee The messaging fee breakdown (nativeFee and lzTokenFee)
    function quote(
        uint32 _dstEid,
        string calldata _message,
        bytes calldata _options,
        bool _payInLzToken
    )
        external
        view
        returns (MessagingFee memory fee)
    {
        bytes memory payload = abi.encode(_message);
        fee = _quote(_dstEid, payload, _options, _payInLzToken);
    }

    /// @notice Returns default execution options for receiving messages
    /// @param _gas The gas limit for lzReceive execution on the destination
    /// @return options The encoded execution options
    /// @dev This is a convenience function for building common options
    function buildOptions(uint128 _gas) external pure returns (bytes memory options) {
        options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(_gas, 0);
    }

    /// @notice Internal handler for receiving messages from other chains
    /// @param _origin The origin information (srcEid, sender, nonce)
    /// @param _guid The unique message identifier
    /// @param _payload The encoded message payload
    /// @dev Called by the LayerZero endpoint after message verification
    function _lzReceive(
        Origin calldata _origin,
        bytes32 _guid,
        bytes calldata _payload,
        address,
        /*_executor*/
        bytes calldata /*_extraData*/
    )
        internal
        override
    {
        string memory message = abi.decode(_payload, (string));

        lastMessage = message;
        lastSrcEid = _origin.srcEid;
        lastSender = _origin.sender;
        messagesReceived++;

        emit MessageReceived(_origin.srcEid, _origin.sender, message, _guid);
    }
}
