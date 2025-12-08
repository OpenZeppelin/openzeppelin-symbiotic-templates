// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

// Use examples/TestOApp for devnet - has ping(), messagesReceived counter
// src/test/TestOApp is minimal version for unit tests
import {TestOApp} from "../src/examples/TestOApp.sol";

/// @title TestOAppDeploy
/// @notice Deploy TestOApp on both source and destination chains
/// @dev Run this twice - once for each chain
contract TestOAppDeploy is Script {
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    address internal deployer;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", address(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266));
    }

    function run() public {
        deployer = getDeployerAddress();

        uint32 chainId = uint32(block.chainid);
        bool isSource = chainId == SOURCE_EID;

        console.log("=== TestOApp Deployment ===");
        console.log("Chain ID:", chainId);
        console.log("Is Source:", isSource);

        // Load endpoint address from LayerZero deployment
        string memory lzFile = isSource
            ? "devnet/deploy-data/lz_source_contracts.json"
            : "devnet/deploy-data/lz_dest_contracts.json";

        string memory json = vm.readFile(lzFile);
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");
        console.log("Endpoint:", endpointAddr);

        vm.startBroadcast(deployer);

        // Deploy TestOApp
        TestOApp oapp = new TestOApp(endpointAddr, deployer);
        console.log("TestOApp:", address(oapp));

        vm.stopBroadcast();

        // Save address
        string memory obj = isSource ? "sourceOApp" : "destOApp";
        string memory outputFile = isSource
            ? "devnet/deploy-data/test_oapp_source.json"
            : "devnet/deploy-data/test_oapp_dest.json";

        string memory finalJson = vm.serializeAddress(obj, "oapp", address(oapp));
        vm.writeJson(finalJson, outputFile);
        console.log("Saved to", outputFile);

        console.log("");
        console.log("=== TestOApp Deployment Complete ===");
    }
}

/// @title TestOAppSetPeers
/// @notice Set peers on both TestOApp contracts after deployment
contract TestOAppSetPeers is Script {
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    address internal deployer;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", address(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266));
    }

    function run() public {
        deployer = getDeployerAddress();

        // Load OApp addresses
        string memory sourceJson = vm.readFile("devnet/deploy-data/test_oapp_source.json");
        string memory destJson = vm.readFile("devnet/deploy-data/test_oapp_dest.json");

        address sourceOApp = vm.parseJsonAddress(sourceJson, ".oapp");
        address destOApp = vm.parseJsonAddress(destJson, ".oapp");

        console.log("=== Setting TestOApp Peers ===");
        console.log("Source OApp:", sourceOApp);
        console.log("Dest OApp:", destOApp);

        uint32 chainId = uint32(block.chainid);
        bool isSource = chainId == SOURCE_EID;

        vm.startBroadcast(deployer);

        if (isSource) {
            // On source chain, set dest as peer
            bytes32 destPeer = bytes32(uint256(uint160(destOApp)));
            TestOApp(payable(sourceOApp)).setPeer(DEST_EID, destPeer);
            console.log("Source OApp peer set to dest:", destOApp);
        } else {
            // On dest chain, set source as peer
            bytes32 sourcePeer = bytes32(uint256(uint160(sourceOApp)));
            TestOApp(payable(destOApp)).setPeer(SOURCE_EID, sourcePeer);
            console.log("Dest OApp peer set to source:", sourceOApp);
        }

        vm.stopBroadcast();

        console.log("=== Peers Set ===");
    }
}
