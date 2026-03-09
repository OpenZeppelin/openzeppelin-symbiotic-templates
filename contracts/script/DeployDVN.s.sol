// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @title DeployDVN
/// @notice Deploy DVN contracts for source and destination chains
/// @dev Run with different RPC URLs for each chain. Requires LayerZero infrastructure deployed first.
///
/// Deployment order:
///   1. Deploy LayerZero infrastructure (DeployLayerZero.s.sol)
///   2. Deploy Relay infrastructure (DeployRelayInfra.s.sol) - includes real Settlement
///   3. Deploy DVN on source: forge script DeployDVN --sig "deploySource(address,uint32)" $SEND_ULN $SOURCE_EID --rpc-url $SOURCE --broadcast
///   4. Deploy DVN on dest: forge script DeployDVN --sig "deployDest(address,address,uint32)" $RECEIVE_ULN $SETTLEMENT $DEST_EID --rpc-url $DEST --broadcast
contract DeployDVN is Script {
    uint256 constant BASE_FEE = 0; // Free for testing

    // Anvil's default deployer
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Deploy DVN on source chain
    /// @param sendUlnAddr Address of SendUln302Mock from LayerZero deployment
    /// @param sourceEid Source LayerZero endpoint ID
    /// @dev Settlement not needed on source, only sendUln for assignJob authorization
    function deploySource(address sendUlnAddr, uint32 sourceEid) external {
        address deployer = msg.sender;

        console.log("=== DVN Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("SendUln302Mock:", sendUlnAddr);

        vm.startBroadcast();

        // Deploy DVN with SendUln302Mock as authorized caller
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            address(0),     // settlement: not needed on source
            sendUlnAddr,    // sendUln: authorized to call assignJob
            address(0),     // receiveUln: not needed on source
            sourceEid,
            BASE_FEE
        );
        console.log("DVN (Source):", address(dvn));

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveSourceContracts(address(dvn), sendUlnAddr);

        console.log("");
        console.log("=== Source Chain Deployment Complete ===");
        console.log("Next: Configure source ULN with DVN address via DeployLayerZero.configureSource()");
    }

    /// @notice Deploy DVN on destination chain
    /// @param receiveUlnAddr Address of ReceiveUln302Mock from LayerZero deployment
    /// @param settlementAddr Address of pre-deployed Settlement contract (from DeployRelayInfra)
    /// @param destEid Destination LayerZero endpoint ID
    function deployDest(address receiveUlnAddr, address settlementAddr, uint32 destEid) external {
        address deployer = msg.sender;
        address submitter = vm.envOr("SUBMITTER_ADDRESS", deployer);

        console.log("=== DVN Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("ReceiveUln302Mock:", receiveUlnAddr);
        console.log("Settlement:", settlementAddr);
        console.log("Submitter:", submitter);

        vm.startBroadcast();

        // Deploy DVN with ReceiveUln302Mock for verify() callback
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            settlementAddr,     // settlement: for BLS verification
            address(0),         // sendUln: not needed on dest
            receiveUlnAddr,     // receiveUln: for verify() callback
            destEid,            // localEid
            BASE_FEE
        );
        console.log("DVN (Dest):", address(dvn));

        // Add submitter (OZ Relayer or deployer for testing)
        dvn.addSubmitter(submitter);
        console.log("Submitter added:", submitter);

        // Add OZ Relayer accounts as authorized submitters
        // Defaults to Anvil accounts 1, 2, 3; overridable via env for external networks
        address submitter1 = vm.envOr("SUBMITTER_1", address(0x70997970C51812dc3A010C7d01b50e0d17dc79C8));
        address submitter2 = vm.envOr("SUBMITTER_2", address(0x90F79bf6EB2c4f870365E785982E1f101E93b906));
        address submitter3 = vm.envOr("SUBMITTER_3", address(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC));

        dvn.addSubmitter(submitter1);
        console.log("OZ Relayer submitter 1 added:", submitter1);

        dvn.addSubmitter(submitter2);
        console.log("OZ Relayer submitter 2 added:", submitter2);

        dvn.addSubmitter(submitter3);
        console.log("OZ Relayer submitter 3 added:", submitter3);

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveDestContracts(address(dvn), receiveUlnAddr, settlementAddr);

        console.log("");
        console.log("=== Destination Chain Deployment Complete ===");
        console.log("Next: Configure dest ULN with DVN address via DeployLayerZero.configureDest()");
    }

    /// @notice Deploy both chains in sequence (for local testing with single script)
    /// @dev Requires SOURCE_RPC and DEST_RPC env vars
    function deployAll() external {
        console.log("=== Full DVN Deployment ===");
        console.log("");

        // This would need vm.createFork() for multi-chain deployment
        // For now, run deploySource() and deployDest() separately
        revert("Use deploySource() and deployDest() separately with different RPC URLs");
    }

    // ============ Internal Helpers ============

    function _saveSourceContracts(address dvn, address sendUln) internal {
        string memory obj = "sourceContracts";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "dvn", dvn);
        string memory json = vm.serializeAddress(obj, "sendUln", sendUln);

        vm.writeJson(json, "deploy-data/source_contracts.json");
        console.log("Saved to deploy-data/source_contracts.json");
    }

    function _saveDestContracts(address dvn, address receiveUln, address settlement) internal {
        string memory obj = "destContracts";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "dvn", dvn);
        vm.serializeAddress(obj, "receiveUln", receiveUln);
        string memory json = vm.serializeAddress(obj, "settlement", settlement);

        vm.writeJson(json, "deploy-data/dest_contracts.json");
        console.log("Saved to deploy-data/dest_contracts.json");
    }
}
