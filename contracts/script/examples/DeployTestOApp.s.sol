// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {TestOApp} from "../../src/examples/TestOApp.sol";

/// @title DeployTestOApp
/// @notice Deploy TestOApp contracts to demonstrate LayerZero cross-chain messaging
/// @dev This script deploys TestOApp on two chains and configures peers for communication
///
/// Prerequisites:
/// - LayerZero endpoints deployed on both chains
/// - DVN infrastructure configured (or use mock endpoint for testing)
///
/// Usage:
///   # Deploy on source chain (e.g., local anvil at port 8545)
///   forge script DeployTestOApp --sig "deploySource(address)" <endpoint> --rpc-url http://localhost:8545 --broadcast
///
///   # Deploy on destination chain (e.g., local anvil at port 8546)
///   forge script DeployTestOApp --sig "deployDest(address)" <endpoint> --rpc-url http://localhost:8546 --broadcast
///
///   # Configure peers after both deployments
///   forge script DeployTestOApp --sig "configurePeers(address,address)" <srcOApp> <dstOApp> --rpc-url <url> --broadcast
contract DeployTestOApp is Script {
    // Chain configurations
    /// @dev For local anvil testing only. In production, chain IDs differ from LayerZero endpoint IDs.
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    // Anvil's default deployer
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Deploy TestOApp on the source chain
    /// @param endpoint Address of the LayerZero endpoint on this chain
    function deploySource(address endpoint) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== TestOApp Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint:", endpoint);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Source):", address(testOApp));

        vm.stopBroadcast();

        _saveSourceContract(address(testOApp), endpoint);

        console.log("");
        console.log("=== Source Deployment Complete ===");
        console.log("Next: Deploy on destination chain with deployDest()");
    }

    /// @notice Deploy TestOApp on the destination chain
    /// @param endpoint Address of the LayerZero endpoint on this chain
    function deployDest(address endpoint) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== TestOApp Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint:", endpoint);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Dest):", address(testOApp));

        vm.stopBroadcast();

        _saveDestContract(address(testOApp), endpoint);

        console.log("");
        console.log("=== Destination Deployment Complete ===");
        console.log("Next: Configure peers with configurePeers()");
    }

    /// @notice Configure peer relationships between source and destination OApps
    /// @param srcOApp Address of TestOApp on source chain
    /// @param dstOApp Address of TestOApp on destination chain
    /// @dev Must be called on both chains to establish bidirectional communication
    function configurePeers(address srcOApp, address dstOApp) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== Configuring OApp Peers ===");
        console.log("Chain ID:", block.chainid);

        vm.startBroadcast(deployer);

        if (block.chainid == SOURCE_EID) {
            // On source chain: set destination as peer
            TestOApp oapp = TestOApp(srcOApp);
            bytes32 dstPeer = bytes32(uint256(uint160(dstOApp)));
            oapp.setPeer(DEST_EID, dstPeer);
            console.log("Source OApp peer set for EID", DEST_EID);
            console.log("  Peer:", dstOApp);
        } else if (block.chainid == DEST_EID) {
            // On destination chain: set source as peer
            TestOApp oapp = TestOApp(dstOApp);
            bytes32 srcPeer = bytes32(uint256(uint160(srcOApp)));
            oapp.setPeer(SOURCE_EID, srcPeer);
            console.log("Dest OApp peer set for EID", SOURCE_EID);
            console.log("  Peer:", srcOApp);
        } else {
            revert("Unknown chain ID - expected SOURCE_EID or DEST_EID");
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== Peer Configuration Complete ===");
    }

    /// @notice Deploy TestOApp on source chain, loading endpoint from LayerZero deployment JSON
    /// @dev Reads endpoint address from deploy-data/layerzero_source.json
    function deploySourceFromJson() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        // Load endpoint address from LayerZero deployment
        string memory json = vm.readFile("deploy-data/layerzero_source.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== TestOApp Source Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Source):", address(testOApp));

        vm.stopBroadcast();

        _saveSourceContract(address(testOApp), endpoint);

        console.log("");
        console.log("=== Source Deployment Complete ===");
        console.log("Next: Deploy on destination chain with deployDestFromJson()");
    }

    /// @notice Deploy TestOApp on destination chain, loading endpoint from LayerZero deployment JSON
    /// @dev Reads endpoint address from deploy-data/layerzero_dest.json
    function deployDestFromJson() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        // Load endpoint address from LayerZero deployment
        string memory json = vm.readFile("deploy-data/layerzero_dest.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== TestOApp Destination Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Dest):", address(testOApp));

        vm.stopBroadcast();

        _saveDestContract(address(testOApp), endpoint);

        console.log("");
        console.log("=== Destination Deployment Complete ===");
        console.log("Next: Configure peers with configurePeers()");
    }

    /// @notice Configure peers using addresses from JSON deployment files
    /// @dev Loads OApp addresses from deploy-data/testoapp_*.json files
    function configurePeersFromJson() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        // Load OApp addresses from deployment JSONs
        string memory srcJson = vm.readFile("deploy-data/testoapp_source.json");
        string memory dstJson = vm.readFile("deploy-data/testoapp_dest.json");
        address srcOApp = vm.parseJsonAddress(srcJson, ".testOApp");
        address dstOApp = vm.parseJsonAddress(dstJson, ".testOApp");

        console.log("=== Configuring OApp Peers (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Source OApp:", srcOApp);
        console.log("Dest OApp:", dstOApp);

        vm.startBroadcast(deployer);

        if (block.chainid == SOURCE_EID) {
            // On source chain: set destination as peer
            TestOApp oapp = TestOApp(srcOApp);
            bytes32 dstPeer = bytes32(uint256(uint160(dstOApp)));
            oapp.setPeer(DEST_EID, dstPeer);
            console.log("Source OApp peer set for EID", DEST_EID);
        } else if (block.chainid == DEST_EID) {
            // On destination chain: set source as peer
            TestOApp oapp = TestOApp(dstOApp);
            bytes32 srcPeer = bytes32(uint256(uint160(srcOApp)));
            oapp.setPeer(SOURCE_EID, srcPeer);
            console.log("Dest OApp peer set for EID", SOURCE_EID);
        } else {
            revert("Unknown chain ID - expected SOURCE_EID or DEST_EID");
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== Peer Configuration Complete ===");
    }

    // ============ Internal Helpers ============

    function _saveSourceContract(address testOApp, address endpoint) internal {
        string memory obj = "sourceTestOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "testOApp", testOApp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.writeJson(json, "deploy-data/testoapp_source.json");
        console.log("Saved to deploy-data/testoapp_source.json");
    }

    function _saveDestContract(address testOApp, address endpoint) internal {
        string memory obj = "destTestOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "testOApp", testOApp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.writeJson(json, "deploy-data/testoapp_dest.json");
        console.log("Saved to deploy-data/testoapp_dest.json");
    }
}

/// @title SendTestMessage
/// @notice Helper script to send a test message via TestOApp
/// @dev Demonstrates how to use the TestOApp to send cross-chain messages
///
/// Usage:
///   forge script SendTestMessage --sig "run(address,uint32,string)" \
///     <testOAppAddress> <dstEid> "Hello from source!" \
///     --rpc-url http://localhost:8545 --broadcast
contract SendTestMessage is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Send a message to another chain
    /// @param testOApp Address of the TestOApp contract
    /// @param dstEid Destination endpoint ID
    /// @param message The message to send
    function run(address testOApp, uint32 dstEid, string calldata message) external {
        address sender = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== Sending Test Message ===");
        console.log("TestOApp:", testOApp);
        console.log("Destination EID:", dstEid);
        console.log("Message:", message);
        console.log("Sender:", sender);

        TestOApp oapp = TestOApp(testOApp);

        // Build options with 200k gas for lzReceive
        bytes memory options = oapp.buildOptions(200_000);

        // Quote the fee
        uint256 fee = oapp.quote(dstEid, message, options, false).nativeFee;
        console.log("Fee (native):", fee);

        vm.startBroadcast(sender);

        // Send the message
        oapp.send{value: fee}(dstEid, message, options);

        vm.stopBroadcast();

        console.log("");
        console.log("=== Message Sent ===");
        console.log("Messages sent total:", oapp.messagesSent());
    }
}
