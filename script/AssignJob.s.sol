// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script, console} from "forge-std/Script.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @notice Script to assign a test job to the DVN
contract AssignJob is Script {
    function run() external {
        address dvnAddr = vm.envAddress("DVN_ADDRESS");
        SymbioticLayerZeroDVN dvn = SymbioticLayerZeroDVN(dvnAddr);

        uint256 fee = dvn.getFee(101, 2, msg.sender, "");
        console.log("Required fee:", fee);

        vm.startBroadcast();

        dvn.assignJob{value: fee}(
            101, // dstEid
            hex"deadbeef", // packetHeader
            bytes32(uint256(0x1234)), // payloadHash
            2, // confirmations
            msg.sender // sender
        );

        console.log("Job assigned!");

        vm.stopBroadcast();
    }
}
