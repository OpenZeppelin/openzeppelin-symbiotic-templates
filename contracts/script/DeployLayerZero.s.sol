// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

// OZ5-compatible mock contracts from test-devtools
import {EndpointV2Mock as EndpointV2} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/EndpointV2Mock.sol";
import {SendUln302Mock as SendUln302} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/SendUln302Mock.sol";
import {ReceiveUln302Mock as ReceiveUln302} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/ReceiveUln302Mock.sol";

// Config structs from messagelib-v2
import {SetDefaultUlnConfigParam, UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {SetDefaultExecutorConfigParam, ExecutorConfig} from
    "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";

import {SimpleExecutor} from "../src/mocks/SimpleExecutor.sol";
import {MockTestHelper} from "../src/mocks/MockTestHelper.sol";

/// @title DeployLayerZero
/// @notice Deploy LayerZero infrastructure using OZ5-compatible mock contracts
/// @dev The actual SendUln302/ReceiveUln302 contracts have OZ4/5 incompatibility,
///      so we use the mock versions from test-devtools which work with OZ5.
///
/// Run with different RPC URLs for source/destination chains:
///   Source: forge script DeployLayerZero --sig "deploySource(uint32)" $SOURCE_EID --rpc-url http://localhost:8545 --broadcast
///   Dest:   forge script DeployLayerZero --sig "deployDest(uint32)" $DEST_EID --rpc-url http://localhost:8546 --broadcast
///
/// Deployment order:
///   1. deploySource(sourceEid) - Deploy EndpointV2, SendUln302Mock, SimpleExecutor on source chain
///   2. deployDest(destEid) - Deploy EndpointV2, ReceiveUln302Mock on destination chain
///   3. Deploy DVN contracts (see DeployDVN.s.sol)
///   4. configureSource(dvnAddr, destEid) - Configure SendUln302Mock with DVN
///   5. configureDest(dvnAddr, sourceEid) - Configure ReceiveUln302Mock with DVN
contract DeployLayerZero is Script {
    // Anvil's default deployer
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    // Treasury gas settings (minimal for testing)
    uint256 constant TREASURY_GAS_CAP = 1000000000000;
    uint256 constant TREASURY_GAS_FOR_FEE_CAP = 100000;

    // ============ Source Chain Deployment ============

    /// @notice Deploy LayerZero infrastructure on source chain
    /// @param sourceEid Source LayerZero endpoint ID for this chain
    /// @dev Deploys EndpointV2, SendUln302Mock, and SimpleExecutor
    ///      Does NOT configure ULN - call configureSource() after DVN is deployed
    function deploySource(uint32 sourceEid) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== LayerZero Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Source EID:", sourceEid);

        vm.startBroadcast(deployer);

        // 1. Deploy MockTestHelper (required by SendUln302Mock for packet scheduling)
        MockTestHelper testHelper = new MockTestHelper();
        console.log("MockTestHelper:", address(testHelper));

        // 2. Deploy EndpointV2Mock
        EndpointV2 endpoint = new EndpointV2(sourceEid, deployer);
        console.log("EndpointV2Mock:", address(endpoint));

        // 3. Deploy SendUln302Mock with testHelper for packet scheduling
        SendUln302 sendUln =
            new SendUln302(payable(address(testHelper)), address(endpoint), TREASURY_GAS_CAP, TREASURY_GAS_FOR_FEE_CAP);
        console.log("SendUln302Mock:", address(sendUln));

        // 4. Register SendUln302Mock as a library with the endpoint
        endpoint.registerLibrary(address(sendUln));
        console.log("SendUln302Mock registered with endpoint");

        // 5. Deploy SimpleExecutor
        SimpleExecutor executor = new SimpleExecutor();
        console.log("SimpleExecutor:", address(executor));

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveSourceInfra(address(endpoint), address(sendUln), address(executor), address(testHelper), sourceEid);

        console.log("");
        console.log("=== Source Chain Deployment Complete ===");
        console.log("Next steps:");
        console.log("  1. Deploy DVN on source chain (DeployDVN.deploySourceWithRealUln)");
        console.log("  2. Run configureSource(dvnAddress) to configure ULN");
    }

    /// @notice Configure SendUln302Mock with DVN and executor
    /// @param dvnAddr Address of the deployed DVN on source chain
    /// @param destEid Destination LayerZero endpoint ID
    function configureSource(address dvnAddr, uint32 destEid) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        // Load deployed addresses
        string memory json = vm.readFile("deploy-data/layerzero_source.json");
        address sendUlnAddr = vm.parseJsonAddress(json, ".sendUln");
        address executorAddr = vm.parseJsonAddress(json, ".executor");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Source Chain ULN ===");
        console.log("SendUln302Mock:", sendUlnAddr);
        console.log("DVN:", dvnAddr);
        console.log("Executor:", executorAddr);

        vm.startBroadcast(deployer);

        SendUln302 sendUln = SendUln302(payable(sendUlnAddr));
        EndpointV2 endpoint = EndpointV2(endpointAddr);

        // 1. Configure default ULN config with DVN
        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddr;
        address[] memory optionalDVNs = new address[](0);

        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: destEid,
            config: UlnConfig({
                confirmations: 1,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: requiredDVNs,
                optionalDVNs: optionalDVNs
            })
        });
        sendUln.setDefaultUlnConfigs(ulnParams);
        console.log("ULN config set with DVN");

        // 2. Configure default executor config
        SetDefaultExecutorConfigParam[] memory execParams = new SetDefaultExecutorConfigParam[](1);
        execParams[0] = SetDefaultExecutorConfigParam({
            eid: destEid,
            config: ExecutorConfig({maxMessageSize: 10000, executor: executorAddr})
        });
        sendUln.setDefaultExecutorConfigs(execParams);
        console.log("Executor config set");

        // 3. Set SendUln302Mock as default send library for destination
        endpoint.setDefaultSendLibrary(destEid, address(sendUln));
        console.log("SendUln302Mock set as default send library for EID", destEid);

        vm.stopBroadcast();

        console.log("");
        console.log("=== Source Chain Configuration Complete ===");
    }

    // ============ Destination Chain Deployment ============

    /// @notice Deploy LayerZero infrastructure on destination chain
    /// @param destEid Destination LayerZero endpoint ID for this chain
    /// @dev Deploys EndpointV2 and ReceiveUln302Mock
    ///      Does NOT configure ULN - call configureDest() after DVN is deployed
    function deployDest(uint32 destEid) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== LayerZero Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Dest EID:", destEid);

        vm.startBroadcast(deployer);

        // 1. Deploy EndpointV2Mock
        EndpointV2 endpoint = new EndpointV2(destEid, deployer);
        console.log("EndpointV2Mock:", address(endpoint));

        // 2. Deploy ReceiveUln302Mock
        ReceiveUln302 receiveUln = new ReceiveUln302(address(endpoint));
        console.log("ReceiveUln302Mock:", address(receiveUln));

        // 3. Register ReceiveUln302Mock as a library with the endpoint
        endpoint.registerLibrary(address(receiveUln));
        console.log("ReceiveUln302Mock registered with endpoint");

        vm.stopBroadcast();

        // Save addresses to JSON
        _saveDestInfra(address(endpoint), address(receiveUln), destEid);

        console.log("");
        console.log("=== Destination Chain Deployment Complete ===");
        console.log("Next steps:");
        console.log("  1. Deploy Settlement on dest chain (DeployDVN.deploySettlement)");
        console.log("  2. Deploy DVN on dest chain (DeployDVN.deployDestWithRealUln)");
        console.log("  3. Run configureDest(dvnAddress) to configure ULN");
    }

    /// @notice Configure ReceiveUln302Mock with DVN
    /// @param dvnAddr Address of the deployed DVN on destination chain
    /// @param sourceEid Source LayerZero endpoint ID
    function configureDest(address dvnAddr, uint32 sourceEid) external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        // Load deployed addresses
        string memory json = vm.readFile("deploy-data/layerzero_dest.json");
        address receiveUlnAddr = vm.parseJsonAddress(json, ".receiveUln");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Destination Chain ULN ===");
        console.log("ReceiveUln302Mock:", receiveUlnAddr);
        console.log("DVN:", dvnAddr);

        vm.startBroadcast(deployer);

        ReceiveUln302 receiveUln = ReceiveUln302(receiveUlnAddr);
        EndpointV2 endpoint = EndpointV2(endpointAddr);

        // 1. Configure default ULN config with DVN
        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddr;
        address[] memory optionalDVNs = new address[](0);

        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: sourceEid,
            config: UlnConfig({
                confirmations: 1,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: requiredDVNs,
                optionalDVNs: optionalDVNs
            })
        });
        receiveUln.setDefaultUlnConfigs(ulnParams);
        console.log("ULN config set with DVN");

        // 2. Set ReceiveUln302Mock as default receive library for source
        endpoint.setDefaultReceiveLibrary(sourceEid, address(receiveUln), 0);
        console.log("ReceiveUln302Mock set as default receive library for EID", sourceEid);

        vm.stopBroadcast();

        console.log("");
        console.log("=== Destination Chain Configuration Complete ===");
    }

    // ============ Internal Helpers ============

    function _saveSourceInfra(address endpoint, address sendUln, address executor, address testHelper, uint32 sourceEid)
        internal
    {
        string memory obj = "sourceInfra";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeUint(obj, "eid", sourceEid);
        vm.serializeAddress(obj, "endpoint", endpoint);
        vm.serializeAddress(obj, "sendUln", sendUln);
        vm.serializeAddress(obj, "executor", executor);
        string memory json = vm.serializeAddress(obj, "testHelper", testHelper);

        vm.writeJson(json, "deploy-data/layerzero_source.json");
        console.log("Saved to deploy-data/layerzero_source.json");
    }

    function _saveDestInfra(address endpoint, address receiveUln, uint32 destEid) internal {
        string memory obj = "destInfra";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeUint(obj, "eid", destEid);
        vm.serializeAddress(obj, "endpoint", endpoint);
        string memory json = vm.serializeAddress(obj, "receiveUln", receiveUln);

        vm.writeJson(json, "deploy-data/layerzero_dest.json");
        console.log("Saved to deploy-data/layerzero_dest.json");
    }
}
