// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {ILayerZeroEndpointV2} from
    "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import {SetConfigParam} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/IMessageLibManager.sol";
import {UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";

/// @title ConfigureExternalOApp
/// @notice Configure per-OApp ULN libraries on real LayerZero V2 endpoints
/// @dev On local anvil, mock contracts accept setDefaultUlnConfigs() (global defaults).
///      On real LZ V2 endpoints, we must configure per-OApp:
///        - endpoint.setSendLibrary(oapp, dstEid, sendUln302)
///        - endpoint.setReceiveLibrary(oapp, srcEid, receiveUln302, gracePeriod)
///        - endpoint.setConfig(oapp, lib, configParams) -- register DVN
///
///      The OApp constructor calls endpoint.setDelegate(owner), so the deployer
///      (OApp owner) is authorized to call these on behalf of the OApp.
contract ConfigureExternalOApp is Script {
    // ULN config type IDs for setConfig
    uint32 constant CONFIG_TYPE_ULN = 2;

    // Anvil's default deployer (fallback)
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Configure source chain: set send library and DVN config for an OApp
    /// @param oappAddr Address of the OApp whose send library to configure
    /// @param dvnAddr Address of the deployed DVN on source chain
    /// @param destEid Destination LayerZero endpoint ID
    function configureSource(address oappAddr, address dvnAddr, uint32 destEid) external {
        address deployer = msg.sender;

        // Load deployed addresses
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

        // On real LZ V2 endpoints, SendUln302 is already the default send library.
        // Calling setSendLibrary() would revert with LZ_OnlyRegisteredOrDefaultLib
        // unless the lib is explicitly registered for this EID pair.
        // We only need to configure the DVN via setConfig.

        // Configure ULN with our DVN via setConfig
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

        console.log("");
        console.log("=== Source Chain Configuration Complete (External) ===");
    }

    /// @notice Configure destination chain: set receive library and DVN config for an OApp
    /// @param oappAddr Address of the OApp whose receive library to configure
    /// @param dvnAddr Address of the deployed DVN on destination chain
    /// @param sourceEid Source LayerZero endpoint ID
    function configureDest(address oappAddr, address dvnAddr, uint32 sourceEid) external {
        address deployer = msg.sender;

        // Load deployed addresses
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

        // On real LZ V2 endpoints, ReceiveUln302 is already the default receive library.
        // We only need to configure the DVN via setConfig.

        // Configure ULN with our DVN via setConfig
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

        console.log("");
        console.log("=== Destination Chain Configuration Complete (External) ===");
    }
}
