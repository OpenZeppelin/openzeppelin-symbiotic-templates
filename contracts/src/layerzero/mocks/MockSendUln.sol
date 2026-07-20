// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ILayerZeroDVN} from "../interfaces/ILayerZeroDVN.sol";

/// @title MockSendUln
/// @notice Mock SendUln302 for testing DVN job assignment
/// @dev Calls DVN.assignJob() when sendMessage is called
contract MockSendUln {
    address public dvn;
    uint32 public immutable localEid;
    address public owner;
    uint64 public nonce;

    event MessageSent(
        bytes32 indexed guid,
        uint32 dstEid,
        address sender,
        bytes message
    );

    event DvnSet(address indexed oldDvn, address indexed newDvn);

    error OnlyOwner();
    error DvnNotSet();

    constructor(uint32 _localEid) {
        localEid = _localEid;
        owner = msg.sender;
    }

    /// @notice Set the DVN address (can only be called by owner)
    function setDvn(address _dvn) external {
        if (msg.sender != owner) revert OnlyOwner();
        address oldDvn = dvn;
        dvn = _dvn;
        emit DvnSet(oldDvn, _dvn);
    }

    /// @notice Send a message and trigger DVN job assignment
    /// @param dstEid Destination endpoint ID
    /// @param receiver Receiver address on destination (as bytes32)
    /// @param message The message payload
    /// @param options Execution options (passed to DVN)
    function sendMessage(
        uint32 dstEid,
        bytes32 receiver,
        bytes calldata message,
        bytes calldata options
    ) external payable returns (bytes32 guid) {
        if (dvn == address(0)) revert DvnNotSet();
        nonce++;

        // Build packet header (81 bytes)
        // version (1) + nonce (8) + srcEid (4) + sender (32) + dstEid (4) + receiver (32)
        bytes memory packetHeader = abi.encodePacked(
            uint8(1),                                    // version
            uint64(nonce),                               // nonce
            localEid,                                    // srcEid
            bytes32(uint256(uint160(msg.sender))),       // sender as bytes32
            dstEid,                                      // dstEid
            receiver                                     // receiver
        );

        bytes32 payloadHash = keccak256(message);
        guid = keccak256(packetHeader);

        // Call DVN to assign verification job
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: dstEid,
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: 1,
            sender: msg.sender
        });

        uint256 fee = ILayerZeroDVN(dvn).assignJob{value: msg.value}(param, options);

        emit MessageSent(guid, dstEid, msg.sender, message);

        // Refund excess
        if (msg.value > fee) {
            payable(msg.sender).transfer(msg.value - fee);
        }

        return guid;
    }

    /// @notice Get fee estimate for sending a message
    function quoteFee(
        uint32 dstEid,
        bytes calldata options
    ) external view returns (uint256) {
        return ILayerZeroDVN(dvn).getFee(dstEid, 1, msg.sender, options);
    }
}
