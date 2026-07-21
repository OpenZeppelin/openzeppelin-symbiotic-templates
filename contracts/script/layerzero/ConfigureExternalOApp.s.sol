// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {ILayerZeroEndpointV2} from
    "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import {SetConfigParam} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/IMessageLibManager.sol";
import {UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";

abstract contract ExternalOAppConfigStep is Script {
    uint32 internal constant CONFIG_TYPE_ULN = 2;

    function _configureExternalSource(address oappAddr, address dvnAddr, uint32 destEid) internal {
        string memory json = vm.readFile("deploy-data/layerzero_source.json");
        address sendUlnAddr = vm.parseJsonAddress(json, ".sendUln");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Source Chain ULN (External) ===");
        console.log("Endpoint:", endpointAddr);
        console.log("OApp:", oappAddr);
        console.log("SendUln302:", sendUlnAddr);
        console.log("DVN:", dvnAddr);
        console.log("Dest EID:", destEid);

        ILayerZeroEndpointV2 endpoint = ILayerZeroEndpointV2(endpointAddr);

        vm.startBroadcast();

        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddr;
        address[] memory optionalDVNs = new address[](0);

        UlnConfig memory ulnConfig = UlnConfig({
            confirmations: 1,
            requiredDVNCount: 1,
            optionalDVNCount: 0,
            optionalDVNThreshold: 0,
            requiredDVNs: requiredDVNs,
            optionalDVNs: optionalDVNs
        });

        SetConfigParam[] memory params = new SetConfigParam[](1);
        params[0] = SetConfigParam({eid: destEid, configType: CONFIG_TYPE_ULN, config: abi.encode(ulnConfig)});

        endpoint.setConfig(oappAddr, sendUlnAddr, params);
        console.log("DVN config set on send library");

        vm.stopBroadcast();
    }

    function _configureExternalDest(address oappAddr, address dvnAddr, uint32 sourceEid) internal {
        string memory json = vm.readFile("deploy-data/layerzero_dest.json");
        address receiveUlnAddr = vm.parseJsonAddress(json, ".receiveUln");
        address endpointAddr = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== Configuring Destination Chain ULN (External) ===");
        console.log("Endpoint:", endpointAddr);
        console.log("OApp:", oappAddr);
        console.log("ReceiveUln302:", receiveUlnAddr);
        console.log("DVN:", dvnAddr);
        console.log("Source EID:", sourceEid);

        ILayerZeroEndpointV2 endpoint = ILayerZeroEndpointV2(endpointAddr);

        vm.startBroadcast();

        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddr;
        address[] memory optionalDVNs = new address[](0);

        UlnConfig memory ulnConfig = UlnConfig({
            confirmations: 1,
            requiredDVNCount: 1,
            optionalDVNCount: 0,
            optionalDVNThreshold: 0,
            requiredDVNs: requiredDVNs,
            optionalDVNs: optionalDVNs
        });

        SetConfigParam[] memory params = new SetConfigParam[](1);
        params[0] = SetConfigParam({eid: sourceEid, configType: CONFIG_TYPE_ULN, config: abi.encode(ulnConfig)});

        endpoint.setConfig(oappAddr, receiveUlnAddr, params);
        console.log("DVN config set on receive library");

        vm.stopBroadcast();
    }
}
