// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {EndpointV2Mock as EndpointV2} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/EndpointV2Mock.sol";
import {SendUln302Mock as SendUln302} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/SendUln302Mock.sol";
import {ReceiveUln302Mock as ReceiveUln302} from
    "@layerzerolabs/test-devtools-evm-foundry/contracts/mocks/ReceiveUln302Mock.sol";

import {SetDefaultUlnConfigParam, UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {SetDefaultExecutorConfigParam, ExecutorConfig} from
    "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";

import {SimpleExecutor} from "../src/mocks/SimpleExecutor.sol";
import {MockTestHelper} from "../src/mocks/MockTestHelper.sol";

abstract contract LayerZeroLocalInfraStep is Script {
    address internal constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    uint256 internal constant TREASURY_GAS_CAP = 1000000000000;
    uint256 internal constant TREASURY_GAS_FOR_FEE_CAP = 100000;

    function _deploySourceInfra(uint32 sourceEid) internal {
        address deployer = _localInfraDeployerAddress();

        console.log("=== LayerZero Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Source EID:", sourceEid);

        _startLocalInfraBroadcast(deployer);

        MockTestHelper testHelper = new MockTestHelper();
        console.log("MockTestHelper:", address(testHelper));

        EndpointV2 endpoint = new EndpointV2(sourceEid, deployer);
        console.log("EndpointV2Mock:", address(endpoint));

        SendUln302 sendUln =
            new SendUln302(payable(address(testHelper)), address(endpoint), TREASURY_GAS_CAP, TREASURY_GAS_FOR_FEE_CAP);
        console.log("SendUln302Mock:", address(sendUln));

        endpoint.registerLibrary(address(sendUln));
        console.log("SendUln302Mock registered with endpoint");

        SimpleExecutor executor = new SimpleExecutor();
        console.log("SimpleExecutor:", address(executor));

        vm.stopBroadcast();

        _saveSourceInfra(address(endpoint), address(sendUln), address(executor), address(testHelper), sourceEid);
    }

    function _configureSourceUln(address dvnAddr, uint32 destEid) internal {
        address deployer = _localInfraDeployerAddress();

        string memory json = vm.readFile("deploy-data/layerzero_source.json");
        address sendUlnAddr = vm.parseJsonAddress(json, ".sendUln");
        address executorAddr = vm.parseJsonAddress(json, ".executor");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Source Chain ULN ===");
        console.log("SendUln302Mock:", sendUlnAddr);
        console.log("DVN:", dvnAddr);
        console.log("Executor:", executorAddr);

        _startLocalInfraBroadcast(deployer);

        SendUln302 sendUln = SendUln302(payable(sendUlnAddr));
        EndpointV2 endpoint = EndpointV2(endpointAddr);

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

        SetDefaultExecutorConfigParam[] memory execParams = new SetDefaultExecutorConfigParam[](1);
        execParams[0] = SetDefaultExecutorConfigParam({
            eid: destEid,
            config: ExecutorConfig({maxMessageSize: 10000, executor: executorAddr})
        });
        sendUln.setDefaultExecutorConfigs(execParams);
        console.log("Executor config set");

        endpoint.setDefaultSendLibrary(destEid, address(sendUln));
        console.log("SendUln302Mock set as default send library for EID", destEid);

        vm.stopBroadcast();
    }

    function _deployDestInfra(uint32 destEid) internal {
        address deployer = _localInfraDeployerAddress();

        console.log("=== LayerZero Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Dest EID:", destEid);

        _startLocalInfraBroadcast(deployer);

        EndpointV2 endpoint = new EndpointV2(destEid, deployer);
        console.log("EndpointV2Mock:", address(endpoint));

        ReceiveUln302 receiveUln = new ReceiveUln302(address(endpoint));
        console.log("ReceiveUln302Mock:", address(receiveUln));

        endpoint.registerLibrary(address(receiveUln));
        console.log("ReceiveUln302Mock registered with endpoint");

        vm.stopBroadcast();

        _saveDestInfra(address(endpoint), address(receiveUln), destEid);
    }

    function _configureDestUln(address dvnAddr, uint32 sourceEid) internal {
        address deployer = _localInfraDeployerAddress();

        string memory json = vm.readFile("deploy-data/layerzero_dest.json");
        address receiveUlnAddr = vm.parseJsonAddress(json, ".receiveUln");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Destination Chain ULN ===");
        console.log("ReceiveUln302Mock:", receiveUlnAddr);
        console.log("DVN:", dvnAddr);

        _startLocalInfraBroadcast(deployer);

        ReceiveUln302 receiveUln = ReceiveUln302(receiveUlnAddr);
        EndpointV2 endpoint = EndpointV2(endpointAddr);

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

        endpoint.setDefaultReceiveLibrary(sourceEid, address(receiveUln), 0);
        console.log("ReceiveUln302Mock set as default receive library for EID", sourceEid);

        vm.stopBroadcast();
    }

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

    function _localInfraDeployerAddress() internal view returns (address) {
        if (vm.envExists("DEPLOYER_ADDRESS")) {
            return vm.envAddress("DEPLOYER_ADDRESS");
        }
        if (vm.envExists("PRIVATE_KEY")) {
            return vm.addr(vm.envUint("PRIVATE_KEY"));
        }
        return DEFAULT_DEPLOYER;
    }

    function _startLocalInfraBroadcast(address deployer) internal {
        if (vm.envExists("PRIVATE_KEY")) {
            vm.startBroadcast(vm.envUint("PRIVATE_KEY"));
        } else {
            vm.startBroadcast(deployer);
        }
    }
}
