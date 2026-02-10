// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";
import {MockCCIPOffRamp} from "../src/mocks/MockCCIPOffRamp.sol";
import {MockCCIPOnRamp} from "../src/mocks/MockCCIPOnRamp.sol";

/// @title DeployCCV
/// @notice Deploy SymbioticCCV contracts on source and destination chains.
/// @dev Both source and destination deployment should use real Settlement contracts
///      deployed by DeployRelayInfra on each respective chain.
contract DeployCCV is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    string constant SOURCE_STORAGE_LOCATION = "mock://symbiotic-ccv/source";
    string constant DEST_STORAGE_LOCATION = "mock://symbiotic-ccv/destination";

    function deploySource(address settlementAddr, uint64 destChainSelector) external {
        if (settlementAddr == address(0)) {
            revert("settlement address required");
        }
        if (destChainSelector == 0) {
            revert("dest chain selector required");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== SymbioticCCV Source Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);

        vm.startBroadcast(deployer);

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = SOURCE_STORAGE_LOCATION;

        SymbioticCCV ccv = new SymbioticCCV(settlementAddr, storageLocations);
        MockCCIPOnRamp onRamp = new MockCCIPOnRamp();
        MockCCIPOffRamp offRamp = new MockCCIPOffRamp(destChainSelector);
        vm.stopBroadcast();

        _saveSourceContracts(address(ccv), settlementAddr, address(onRamp), address(offRamp));

        console.log("Source settlement:", settlementAddr);
        console.log("Source SymbioticCCV:", address(ccv));
        console.log("Source mock OnRamp:", address(onRamp));
        console.log("Source mock OffRamp:", address(offRamp));
        console.log("Saved to deploy-data/ccv_source_contracts.json");
    }

    function deployDest(address settlementAddr, uint64 sourceChainSelector) external {
        if (settlementAddr == address(0)) {
            revert("settlement address required");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== SymbioticCCV Destination Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);

        vm.startBroadcast(deployer);

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = DEST_STORAGE_LOCATION;

        SymbioticCCV ccv = new SymbioticCCV(settlementAddr, storageLocations);
        MockCCIPOnRamp onRamp = new MockCCIPOnRamp();
        MockCCIPOffRamp offRamp = new MockCCIPOffRamp(sourceChainSelector);
        vm.stopBroadcast();

        _saveDestContracts(address(ccv), settlementAddr, address(onRamp), address(offRamp));

        console.log("Dest SymbioticCCV:", address(ccv));
        console.log("Dest mock OnRamp:", address(onRamp));
        console.log("Dest mock OffRamp:", address(offRamp));
        console.log("Saved to deploy-data/ccv_dest_contracts.json");
    }

    function _saveSourceContracts(address ccv, address settlement, address onRamp, address offRamp) internal {
        string memory obj = "sourceCCV";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "ccv", ccv);
        vm.serializeAddress(obj, "settlement", settlement);
        vm.serializeAddress(obj, "onRamp", onRamp);
        string memory json = vm.serializeAddress(obj, "offRamp", offRamp);

        vm.writeJson(json, "deploy-data/ccv_source_contracts.json");
    }

    function _saveDestContracts(address ccv, address settlement, address onRamp, address offRamp) internal {
        string memory obj = "destCCV";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "ccv", ccv);
        vm.serializeAddress(obj, "settlement", settlement);
        vm.serializeAddress(obj, "onRamp", onRamp);
        string memory json = vm.serializeAddress(obj, "offRamp", offRamp);

        vm.writeJson(json, "deploy-data/ccv_dest_contracts.json");
    }
}
