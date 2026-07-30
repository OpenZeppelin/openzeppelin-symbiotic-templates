// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/// @title MockTestHelper
/// @notice Minimal mock for TestHelperOz5.schedulePacket() used by SendUln302Mock
/// @dev Simply stores scheduled packets for later manual processing
contract MockTestHelper {
    struct ScheduledPacket {
        bytes packet;
        bytes options;
        uint256 timestamp;
    }

    ScheduledPacket[] public scheduledPackets;

    event PacketScheduled(uint256 indexed index, bytes packet, bytes options);

    /// @notice Called by SendUln302Mock to schedule a packet for delivery
    function schedulePacket(bytes calldata _packet, bytes calldata _options) external payable {
        scheduledPackets.push(ScheduledPacket({packet: _packet, options: _options, timestamp: block.timestamp}));

        emit PacketScheduled(scheduledPackets.length - 1, _packet, _options);
    }

    /// @notice Get the number of scheduled packets
    function getScheduledPacketCount() external view returns (uint256) {
        return scheduledPackets.length;
    }

    /// @notice Get a scheduled packet by index
    function getScheduledPacket(uint256 _index) external view returns (bytes memory, bytes memory, uint256) {
        ScheduledPacket memory p = scheduledPackets[_index];
        return (p.packet, p.options, p.timestamp);
    }
}
