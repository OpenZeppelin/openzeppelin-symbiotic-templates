// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {EndpointV2} from "@layerzerolabs/lz-evm-protocol-v2/contracts/EndpointV2.sol";
import {SendUln302} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/uln302/SendUln302.sol";
import {SetDefaultUlnConfigParam, UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {ILayerZeroEndpointV2} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import {SetDefaultExecutorConfigParam, ExecutorConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";

import {SimpleExecutor} from "../src/test/SimpleExecutor.sol";

/// @title LayerZeroSourceDeploy
/// @notice Deploy EndpointV2 and SendUln302 on source chain (31337)
/// @dev Reads DVN address from source_chain_contracts.json
contract LayerZeroSourceDeploy is Script {
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    address internal deployer;
    address internal dvnAddress;

    EndpointV2 internal endpoint;
    SendUln302 internal sendUln;
    SimpleExecutor internal executor;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", address(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266));
    }

    function run() public {
        deployer = getDeployerAddress();

        console.log("=== LayerZero Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);

        // Load DVN address from previous deployment
        string memory json = vm.readFile("devnet/deploy-data/source_chain_contracts.json");
        dvnAddress = vm.parseJsonAddress(json, ".dvn.addr");
        console.log("DVN Address:", dvnAddress);

        vm.startBroadcast(deployer);

        // 1. Deploy EndpointV2
        endpoint = new EndpointV2(SOURCE_EID, deployer);
        console.log("EndpointV2:", address(endpoint));

        // 2. Deploy SendUln302
        sendUln = new SendUln302(
            address(endpoint),
            0, // treasuryGasLimit
            0  // treasuryGasForFeeCap
        );
        console.log("SendUln302:", address(sendUln));

        // 3. Register SendUln302 as a library
        endpoint.registerLibrary(address(sendUln));
        console.log("SendUln302 registered as library");

        // 4. Configure DVN in SendUln302 (MUST be before setDefaultSendLibrary!)
        // An EID is only "supported" if it has a ULN config with at least one DVN
        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddress;

        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: DEST_EID,
            config: UlnConfig({
                confirmations: 1,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: requiredDVNs,
                optionalDVNs: new address[](0)
            })
        });
        sendUln.setDefaultUlnConfigs(ulnParams);
        console.log("SendUln302 configured with DVN:", dvnAddress);

        // 5. Deploy SimpleExecutor
        executor = new SimpleExecutor();
        console.log("SimpleExecutor:", address(executor));

        // 6. Configure Executor in SendUln302
        SetDefaultExecutorConfigParam[] memory execParams = new SetDefaultExecutorConfigParam[](1);
        execParams[0] = SetDefaultExecutorConfigParam({
            eid: DEST_EID,
            config: ExecutorConfig({
                maxMessageSize: 10000,
                executor: address(executor)
            })
        });
        sendUln.setDefaultExecutorConfigs(execParams);
        console.log("SendUln302 configured with Executor:", address(executor));

        // 7. Set SendUln302 as default send library for destination
        endpoint.setDefaultSendLibrary(DEST_EID, address(sendUln));
        console.log("SendUln302 set as default send library for EID", DEST_EID);

        vm.stopBroadcast();

        // Save addresses
        saveAddresses();

        console.log("");
        console.log("=== LayerZero Source Deploy Complete ===");
    }

    function saveAddresses() internal {
        string memory obj = "lzSource";

        vm.serializeAddress(obj, "endpoint", address(endpoint));
        vm.serializeAddress(obj, "sendUln", address(sendUln));
        string memory finalJson = vm.serializeAddress(obj, "executor", address(executor));

        vm.writeJson(finalJson, "devnet/deploy-data/lz_source_contracts.json");
        console.log("Saved to devnet/deploy-data/lz_source_contracts.json");
    }
}
