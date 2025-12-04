// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {EndpointV2} from "@layerzerolabs/lz-evm-protocol-v2/contracts/EndpointV2.sol";
import {ReceiveUln302} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/uln302/ReceiveUln302.sol";
import {SetDefaultUlnConfigParam, UlnConfig} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {ILayerZeroEndpointV2} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";

import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @title LayerZeroDestDeploy
/// @notice Deploy EndpointV2 and ReceiveUln302 on destination chain (31338)
/// @dev Reads DVN address from dest_chain_contracts.json and configures it
contract LayerZeroDestDeploy is Script {
    uint32 constant SOURCE_EID = 31337;
    uint32 constant DEST_EID = 31338;

    address internal deployer;
    address internal dvnAddress;

    EndpointV2 internal endpoint;
    ReceiveUln302 internal receiveUln;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", address(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266));
    }

    function run() public {
        deployer = getDeployerAddress();

        console.log("=== LayerZero Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);

        // Load DVN address from previous deployment
        string memory json = vm.readFile("devnet/deploy-data/dest_chain_contracts.json");
        dvnAddress = vm.parseJsonAddress(json, ".dvn.addr");
        console.log("DVN Address:", dvnAddress);

        vm.startBroadcast(deployer);

        // 1. Deploy EndpointV2
        endpoint = new EndpointV2(DEST_EID, deployer);
        console.log("EndpointV2:", address(endpoint));

        // 2. Deploy ReceiveUln302
        receiveUln = new ReceiveUln302(address(endpoint));
        console.log("ReceiveUln302:", address(receiveUln));

        // 3. Register ReceiveUln302 as a library
        endpoint.registerLibrary(address(receiveUln));
        console.log("ReceiveUln302 registered as library");

        // 4. Configure DVN in ReceiveUln302 (MUST be before setDefaultReceiveLibrary!)
        // An EID is only "supported" if it has a ULN config with at least one DVN
        address[] memory requiredDVNs = new address[](1);
        requiredDVNs[0] = dvnAddress;

        SetDefaultUlnConfigParam[] memory ulnParams = new SetDefaultUlnConfigParam[](1);
        ulnParams[0] = SetDefaultUlnConfigParam({
            eid: SOURCE_EID,
            config: UlnConfig({
                confirmations: 1,
                requiredDVNCount: 1,
                optionalDVNCount: 0,
                optionalDVNThreshold: 0,
                requiredDVNs: requiredDVNs,
                optionalDVNs: new address[](0)
            })
        });
        receiveUln.setDefaultUlnConfigs(ulnParams);
        console.log("ReceiveUln302 configured with DVN:", dvnAddress);

        // 5. Set ReceiveUln302 as default receive library for source chain
        endpoint.setDefaultReceiveLibrary(SOURCE_EID, address(receiveUln), 0);
        console.log("ReceiveUln302 set as default receive library for EID", SOURCE_EID);

        // 6. Configure DVN with ReceiveUln302 address
        SymbioticLayerZeroDVN(dvnAddress).setReceiveUln(address(receiveUln));
        console.log("DVN configured with ReceiveUln302:", address(receiveUln));

        vm.stopBroadcast();

        // Save addresses
        saveAddresses();

        console.log("");
        console.log("=== LayerZero Destination Deploy Complete ===");
    }

    function saveAddresses() internal {
        string memory obj = "lzDest";

        vm.serializeAddress(obj, "endpoint", address(endpoint));
        string memory finalJson = vm.serializeAddress(obj, "receiveUln", address(receiveUln));

        vm.writeJson(finalJson, "devnet/deploy-data/lz_dest_contracts.json");
        console.log("Saved to devnet/deploy-data/lz_dest_contracts.json");
    }
}
