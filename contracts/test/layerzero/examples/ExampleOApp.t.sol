// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { Test } from "forge-std/Test.sol";
import { console } from "forge-std/console.sol";

import { ExampleOApp } from "../../../src/layerzero/ExampleOApp.sol";
import { OptionsBuilder } from "@layerzerolabs/oapp-evm/contracts/oapp/libs/OptionsBuilder.sol";
import { Origin } from "@layerzerolabs/oapp-evm/contracts/oapp/OApp.sol";
import {
    MessagingFee,
    MessagingReceipt
} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";

/// @title MockEndpointV2
/// @notice Minimal mock of LayerZero EndpointV2 for testing OApp contracts
/// @dev Only implements the functions needed by ExampleOApp
contract MockEndpointV2 {
    uint32 public immutable eid;
    mapping(address => address) public delegates;

    uint64 public nonce;
    address public lzToken;

    // Recorded sends for verification
    struct SentMessage {
        uint32 dstEid;
        bytes32 receiver;
        bytes message;
        bytes options;
        uint256 fee;
    }
    SentMessage[] public sentMessages;

    event MessageSent(uint32 indexed dstEid, bytes32 receiver, bytes message);

    constructor(uint32 _eid) {
        eid = _eid;
    }

    function setDelegate(address _delegate) external {
        delegates[msg.sender] = _delegate;
    }

    function quote(
        MessagingParams memory,
        /*_params*/
        address /*_sender*/
    )
        external
        pure
        returns (MessagingFee memory)
    {
        // Return a fixed fee for testing
        return MessagingFee({ nativeFee: 0.001 ether, lzTokenFee: 0 });
    }

    function send(
        MessagingParams memory _params,
        address _refundAddress
    )
        external
        payable
        returns (MessagingReceipt memory receipt)
    {
        nonce++;

        // Record the sent message
        sentMessages.push(
            SentMessage({
                dstEid: _params.dstEid,
                receiver: _params.receiver,
                message: _params.message,
                options: _params.options,
                fee: msg.value
            })
        );

        emit MessageSent(_params.dstEid, _params.receiver, _params.message);

        // Return receipt
        receipt = MessagingReceipt({
            guid: keccak256(abi.encodePacked(block.timestamp, nonce, _params.dstEid)),
            nonce: nonce,
            fee: MessagingFee({ nativeFee: msg.value, lzTokenFee: 0 })
        });

        // Refund excess
        uint256 excess = msg.value > 0.001 ether ? msg.value - 0.001 ether : 0;
        if (excess > 0) {
            payable(_refundAddress).transfer(excess);
        }
    }

    function getSentMessagesCount() external view returns (uint256) {
        return sentMessages.length;
    }
}

// Struct needed by endpoint.quote and endpoint.send
struct MessagingParams {
    uint32 dstEid;
    bytes32 receiver;
    bytes message;
    bytes options;
    bool payInLzToken;
}

/// @title ExampleOAppTest
/// @notice Unit tests for the ExampleOApp starter contract
/// @dev Tests basic functionality without full LayerZero infrastructure
contract ExampleOAppTest is Test {
    using OptionsBuilder for bytes;

    ExampleOApp public sourceOApp;
    ExampleOApp public destOApp;
    MockEndpointV2 public sourceEndpoint;
    MockEndpointV2 public destEndpoint;

    uint32 constant SOURCE_EID = 31_337;
    uint32 constant DEST_EID = 31_338;

    address public owner;
    address public user;

    function setUp() public {
        owner = address(this);
        user = makeAddr("user");
        vm.deal(user, 10 ether);

        // Deploy mock endpoints
        sourceEndpoint = new MockEndpointV2(SOURCE_EID);
        destEndpoint = new MockEndpointV2(DEST_EID);

        // Deploy OApps
        sourceOApp = new ExampleOApp(address(sourceEndpoint), owner);
        destOApp = new ExampleOApp(address(destEndpoint), owner);

        // Configure peers
        sourceOApp.setPeer(DEST_EID, bytes32(uint256(uint160(address(destOApp)))));
        destOApp.setPeer(SOURCE_EID, bytes32(uint256(uint160(address(sourceOApp)))));
    }

    function test_constructor() public view {
        assertEq(address(sourceOApp.endpoint()), address(sourceEndpoint));
        assertEq(address(destOApp.endpoint()), address(destEndpoint));
        assertEq(sourceOApp.owner(), owner);
        assertEq(destOApp.owner(), owner);
    }

    function test_setPeer() public view {
        bytes32 expectedPeer = bytes32(uint256(uint160(address(destOApp))));
        assertEq(sourceOApp.peers(DEST_EID), expectedPeer);
    }

    function test_buildOptions() public view {
        bytes memory options = sourceOApp.buildOptions(200_000);
        assertGt(options.length, 0);
    }

    function test_quote() public view {
        string memory message = "Hello, LayerZero!";
        bytes memory options = sourceOApp.buildOptions(200_000);

        MessagingFee memory fee = sourceOApp.quote(DEST_EID, message, options, false);

        assertEq(fee.nativeFee, 0.001 ether);
        assertEq(fee.lzTokenFee, 0);
    }

    function test_send() public {
        string memory message = "Hello, LayerZero!";
        bytes memory options = sourceOApp.buildOptions(200_000);

        MessagingFee memory fee = sourceOApp.quote(DEST_EID, message, options, false);

        vm.prank(user);
        MessagingReceipt memory receipt = sourceOApp.send{ value: fee.nativeFee }(DEST_EID, message, options);

        // Verify receipt
        assertGt(receipt.nonce, 0);
        assertEq(receipt.fee.nativeFee, fee.nativeFee);

        // Verify counter
        assertEq(sourceOApp.messagesSent(), 1);

        // Verify message was recorded by mock endpoint
        assertEq(sourceEndpoint.getSentMessagesCount(), 1);
    }

    function test_send_multipleTimes() public {
        bytes memory options = sourceOApp.buildOptions(200_000);
        uint256 fee = 0.001 ether;

        vm.startPrank(user);

        sourceOApp.send{ value: fee }(DEST_EID, "Message 1", options);
        sourceOApp.send{ value: fee }(DEST_EID, "Message 2", options);
        sourceOApp.send{ value: fee }(DEST_EID, "Message 3", options);

        vm.stopPrank();

        assertEq(sourceOApp.messagesSent(), 3);
        assertEq(sourceEndpoint.getSentMessagesCount(), 3);
    }

    function test_lzReceive() public view {
        // Verify initial state before any messages are received
        // Note: _lzReceive is internal, so we test via ExampleOAppHarness in ExampleOAppReceiveTest
        assertEq(destOApp.messagesReceived(), 0);
        assertEq(bytes(destOApp.lastMessage()).length, 0);
    }

    function test_oAppVersion() public view {
        (uint64 senderVersion, uint64 receiverVersion) = sourceOApp.oAppVersion();
        assertGt(senderVersion, 0);
        assertGt(receiverVersion, 0);
    }

    function test_setDelegate() public {
        address newDelegate = makeAddr("newDelegate");

        sourceOApp.setDelegate(newDelegate);

        // Verify delegate was set in the endpoint
        assertEq(sourceEndpoint.delegates(address(sourceOApp)), newDelegate);
    }

    function test_send_withExactFee() public {
        // Test that sending with exact fee works correctly
        string memory message = "Hello!";
        bytes memory options = sourceOApp.buildOptions(200_000);
        uint256 fee = 0.001 ether;

        vm.prank(user);
        MessagingReceipt memory receipt = sourceOApp.send{ value: fee }(DEST_EID, message, options);

        assertEq(receipt.fee.nativeFee, fee);
        assertEq(sourceOApp.messagesSent(), 1);
    }

    function test_send_emitsEvent() public {
        string memory message = "Test event emission";
        bytes memory options = sourceOApp.buildOptions(200_000);
        uint256 fee = 0.001 ether;

        vm.prank(user);
        vm.expectEmit(true, false, false, false);
        emit ExampleOApp.MessageSent(DEST_EID, message, bytes32(0), 0);
        sourceOApp.send{ value: fee }(DEST_EID, message, options);
    }
}

/// @title ExampleOAppHarness
/// @notice Test harness to expose internal functions for testing
contract ExampleOAppHarness is ExampleOApp {
    constructor(address _endpoint, address _delegate) ExampleOApp(_endpoint, _delegate) { }

    /// @notice Expose _lzReceive for testing
    function exposed_lzReceive(
        Origin calldata _origin,
        bytes32 _guid,
        bytes calldata _payload,
        address _executor,
        bytes calldata _extraData
    )
        external
    {
        _lzReceive(_origin, _guid, _payload, _executor, _extraData);
    }
}

/// @title ExampleOAppReceiveTest
/// @notice Tests for message receiving functionality using a test harness
contract ExampleOAppReceiveTest is Test {
    ExampleOAppHarness public oapp;
    MockEndpointV2 public endpoint;

    uint32 constant LOCAL_EID = 31_338;
    uint32 constant REMOTE_EID = 31_337;

    address public owner;

    function setUp() public {
        owner = address(this);
        endpoint = new MockEndpointV2(LOCAL_EID);
        oapp = new ExampleOAppHarness(address(endpoint), owner);

        // Set peer
        oapp.setPeer(REMOTE_EID, bytes32(uint256(uint160(address(0xBEEF)))));
    }

    function test_lzReceive_storesMessage() public {
        string memory message = "Hello from remote chain!";
        bytes memory payload = abi.encode(message);
        bytes32 sender = bytes32(uint256(uint160(address(0xBEEF))));
        bytes32 guid = keccak256("test-guid-123");

        Origin memory origin = Origin({ srcEid: REMOTE_EID, sender: sender, nonce: 1 });

        oapp.exposed_lzReceive(origin, guid, payload, address(0), "");

        assertEq(oapp.lastMessage(), message);
        assertEq(oapp.lastSrcEid(), REMOTE_EID);
        assertEq(oapp.lastSender(), sender);
        assertEq(oapp.messagesReceived(), 1);
    }

    function test_lzReceive_multipleMessages() public {
        bytes32 sender = bytes32(uint256(uint160(address(0xBEEF))));
        Origin memory origin = Origin({ srcEid: REMOTE_EID, sender: sender, nonce: 1 });

        // Receive first message
        oapp.exposed_lzReceive(origin, keccak256("guid-1"), abi.encode("First"), address(0), "");
        assertEq(oapp.lastMessage(), "First");
        assertEq(oapp.messagesReceived(), 1);

        // Receive second message
        origin.nonce = 2;
        oapp.exposed_lzReceive(origin, keccak256("guid-2"), abi.encode("Second"), address(0), "");
        assertEq(oapp.lastMessage(), "Second");
        assertEq(oapp.messagesReceived(), 2);
    }

    function test_lzReceive_emitsEvent() public {
        string memory message = "Event test";
        bytes memory payload = abi.encode(message);
        bytes32 sender = bytes32(uint256(uint160(address(0xBEEF))));
        bytes32 guid = keccak256("event-test-guid");

        Origin memory origin = Origin({ srcEid: REMOTE_EID, sender: sender, nonce: 1 });

        vm.expectEmit(true, false, false, true);
        emit ExampleOApp.MessageReceived(REMOTE_EID, sender, message, guid);

        oapp.exposed_lzReceive(origin, guid, payload, address(0), "");
    }
}
