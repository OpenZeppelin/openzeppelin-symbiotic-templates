// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import "forge-std/Test.sol";
import {TestOApp} from "../src/examples/TestOApp.sol";
import {OptionsBuilder} from "@layerzerolabs/lz-evm-oapp-v2/contracts/oapp/libs/OptionsBuilder.sol";

/// @title TestOApp Unit Tests
/// @notice Tests the TestOApp contract functionality without full LayerZero integration
contract TestOAppTest is Test {
    using OptionsBuilder for bytes;

    TestOApp oapp;
    MockEndpoint mockEndpoint;

    address deployer = address(0x1);
    address user = address(0x2);

    uint32 constant LOCAL_EID = 31337;
    uint32 constant REMOTE_EID = 31338;

    function setUp() public {
        vm.startPrank(deployer);

        // Deploy mock endpoint
        mockEndpoint = new MockEndpoint(LOCAL_EID);

        // Deploy TestOApp
        oapp = new TestOApp(address(mockEndpoint), deployer);

        // Set peer for remote chain
        oapp.setPeer(REMOTE_EID, bytes32(uint256(uint160(address(0xDEAD)))));

        vm.stopPrank();
    }

    /// @notice Test that OApp is initialized correctly
    function testInitialization() public view {
        assertEq(address(oapp.endpoint()), address(mockEndpoint));
        assertEq(oapp.owner(), deployer);
        assertEq(oapp.messagesReceived(), 0);
    }

    /// @notice Test peer configuration
    function testPeerConfiguration() public view {
        assertTrue(oapp.hasPeer(REMOTE_EID));
        assertFalse(oapp.hasPeer(99999));
    }

    /// @notice Test setting peer
    function testSetPeer() public {
        vm.prank(deployer);
        oapp.setPeer(12345, bytes32(uint256(0x1234)));
        assertTrue(oapp.hasPeer(12345));
    }

    /// @notice Test only owner can set peer
    function testSetPeerOnlyOwner() public {
        vm.prank(user);
        vm.expectRevert();
        oapp.setPeer(12345, bytes32(uint256(0x1234)));
    }

    /// @notice Test quote functionality
    function testQuote() public {
        bytes memory message = abi.encode("Hello");
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(200000, 0);

        // Quote should not revert (mock returns 0 fee)
        oapp.quote(REMOTE_EID, message, options);
    }

    /// @notice Test quotePing functionality
    function testQuotePing() public {
        // QuotePing should not revert
        oapp.quotePing(REMOTE_EID);
    }

    /// @notice Test that send requires peer to be set
    function testSendRequiresPeer() public {
        bytes memory message = abi.encode("Hello");
        bytes memory options = OptionsBuilder.newOptions().addExecutorLzReceiveOption(200000, 0);

        vm.prank(user);
        vm.deal(user, 1 ether);

        // Try to send to chain without peer
        vm.expectRevert("Peer not set");
        oapp.send{value: 0.1 ether}(99999, message, options);
    }

    /// @notice Test receiving messages
    function testLzReceiveCountsMessages() public {
        // Simulate receiving a message by calling through mock endpoint
        assertEq(oapp.messagesReceived(), 0);

        // Direct lzReceive call (simulating what endpoint would do)
        vm.prank(address(mockEndpoint));
        mockEndpoint.simulateLzReceive(
            payable(address(oapp)),
            REMOTE_EID,
            bytes32(uint256(uint160(address(0xDEAD)))),
            1,
            abi.encode("Hello from remote!")
        );

        assertEq(oapp.messagesReceived(), 1);
        assertEq(oapp.lastSrcEid(), REMOTE_EID);
    }

    /// @notice Test multiple message receives
    function testMultipleMessagesReceived() public {
        vm.startPrank(address(mockEndpoint));

        for (uint256 i = 0; i < 5; i++) {
            mockEndpoint.simulateLzReceive(
                payable(address(oapp)),
                REMOTE_EID,
                bytes32(uint256(uint160(address(0xDEAD)))),
                uint64(i + 1),
                abi.encode("Message", i)
            );
        }

        vm.stopPrank();

        assertEq(oapp.messagesReceived(), 5);
    }
}

/// @notice Mock LayerZero Endpoint for testing
contract MockEndpoint {
    uint32 public eid;

    struct MessagingFee {
        uint256 nativeFee;
        uint256 lzTokenFee;
    }

    struct MessagingReceipt {
        bytes32 guid;
        uint64 nonce;
        MessagingFee fee;
    }

    struct MessagingParams {
        uint32 dstEid;
        bytes32 receiver;
        bytes message;
        bytes options;
        bool payInLzToken;
    }

    constructor(uint32 _eid) {
        eid = _eid;
    }

    /// @notice Mock setDelegate
    function setDelegate(address) external {}

    /// @notice Mock quote function - matches ILayerZeroEndpointV2 interface
    function quote(
        MessagingParams calldata,
        address
    ) external pure returns (MessagingFee memory) {
        return MessagingFee(0.001 ether, 0);
    }

    /// @notice Mock send function - matches ILayerZeroEndpointV2 interface
    function send(
        MessagingParams calldata,
        address
    ) external payable returns (MessagingReceipt memory) {
        return MessagingReceipt(bytes32(0), 1, MessagingFee(msg.value, 0));
    }

    /// @notice Simulate lzReceive for testing
    function simulateLzReceive(
        address payable _receiver,
        uint32 _srcEid,
        bytes32 _sender,
        uint64 _nonce,
        bytes calldata _message
    ) external {
        TestOApp(_receiver).mockReceive(_srcEid, _sender, _nonce, _message);
    }
}
