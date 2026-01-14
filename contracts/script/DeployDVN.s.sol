// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";
import {MockSendUln} from "../src/mocks/MockSendUln.sol";
import {MockReceiveUln} from "../src/mocks/MockReceiveUln.sol";

/// @title DeployDVN
/// @notice Deploy DVN contracts for source and destination chains
/// @dev Run with different RPC URLs for each chain:
///   Source: forge script DeployDVN --sig "deploySource()" --rpc-url http://localhost:8545 --broadcast
///   Dest:   forge script DeployDVN --sig "deployDest(address)" <settlement> --rpc-url http://localhost:8546 --broadcast
contract DeployDVN is Script {
    // Chain configurations
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;
    uint256 constant BASE_FEE = 0; // Free for testing

    // Anvil's default deployer
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Deploy on source chain (31337)
    /// @dev Settlement not needed on source, only sendUln
    ///      Uses simple two-phase deployment (no CREATE2 prediction needed)
    function deploySource() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== DVN Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        // Step 1: Deploy MockSendUln first (DVN address will be set later)
        MockSendUln sendUln = new MockSendUln(SOURCE_EID);
        console.log("MockSendUln:", address(sendUln));

        // Step 2: Deploy DVN with MockSendUln as authorized sendUln caller
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            address(0),         // settlement: not needed on source
            address(sendUln),   // sendUln: MockSendUln is authorized!
            address(0),         // receiveUln: not needed on source
            SOURCE_EID,
            BASE_FEE
        );
        console.log("DVN (Source):", address(dvn));

        // Step 3: Set DVN address in MockSendUln
        sendUln.setDvn(address(dvn));
        console.log("MockSendUln.dvn set to:", address(dvn));

        // Verify MockSendUln points to the correct DVN
        require(sendUln.dvn() == address(dvn), "MockSendUln DVN mismatch");

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveSourceContracts(address(dvn), address(sendUln));

        console.log("");
        console.log("=== Source Chain Deployment Complete ===");
        console.log("Next: Deploy relay infrastructure on destination chain");
    }

    /// @notice Deploy on destination chain (31338)
    /// @param settlementAddr Address of pre-deployed Settlement contract
    function deployDest(address settlementAddr) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);
        address submitter = vm.envOr("SUBMITTER_ADDRESS", deployer);

        console.log("=== DVN Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);
        console.log("Submitter:", submitter);

        vm.startBroadcast(deployer);

        // 1. Deploy MockReceiveUln first
        MockReceiveUln receiveUln = new MockReceiveUln();
        console.log("MockReceiveUln:", address(receiveUln));

        // 2. Deploy DVN (destination chain - needs settlement + receiveUln)
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            settlementAddr,         // settlement: for BLS verification
            address(0),             // sendUln: not needed on dest
            address(receiveUln),    // receiveUln: for verify() callback
            DEST_EID,               // localEid
            BASE_FEE
        );
        console.log("DVN (Dest):", address(dvn));

        // 3. Add submitter (OZ Relayer or deployer for testing)
        dvn.addSubmitter(submitter);
        console.log("Submitter added:", submitter);

        // Add OZ Relayer as authorized submitter (Anvil account 1)
        dvn.addSubmitter(0x70997970C51812dc3A010C7d01b50e0d17dc79C8);
        console.log("OZ Relayer submitter added:", 0x70997970C51812dc3A010C7d01b50e0d17dc79C8);

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveDestContracts(address(dvn), address(receiveUln), settlementAddr);

        console.log("");
        console.log("=== Destination Chain Deployment Complete ===");
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

/// @title DeployDVNWithMockSendUln
/// @notice Deploy DVN where MockSendUln is the authorized sendUln caller
/// @dev Use this for testing where MockSendUln.sendMessage() triggers DVN.assignJob()
///      Uses simple two-phase deployment (no CREATE2 prediction needed)
contract DeployDVNWithMockSendUln is Script {
    uint32 constant SOURCE_EID = 31337;
    uint256 constant BASE_FEE = 0;
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    function run() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== DVN + MockSendUln Deployment ===");
        console.log("Chain ID:", block.chainid);

        vm.startBroadcast(deployer);

        // Step 1: Deploy MockSendUln first (DVN address will be set later)
        MockSendUln sendUln = new MockSendUln(SOURCE_EID);
        console.log("MockSendUln:", address(sendUln));

        // Step 2: Deploy DVN with MockSendUln as authorized caller
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            address(0),         // settlement
            address(sendUln),   // sendUln: MockSendUln is authorized!
            address(0),         // receiveUln
            SOURCE_EID,
            BASE_FEE
        );
        console.log("DVN:", address(dvn));

        // Step 3: Set DVN address in MockSendUln
        sendUln.setDvn(address(dvn));
        console.log("MockSendUln.dvn set to:", address(dvn));

        // Verify MockSendUln points to the correct DVN
        require(sendUln.dvn() == address(dvn), "MockSendUln DVN mismatch");

        vm.stopBroadcast();

        console.log("");
        console.log("MockSendUln.sendMessage() will call DVN.assignJob()");
    }
}
