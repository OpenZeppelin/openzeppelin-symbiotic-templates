// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";
import {MockCCIPOffRamp} from "../src/mocks/MockCCIPOffRamp.sol";
import {MockCCIPOnRamp} from "../src/mocks/MockCCIPOnRamp.sol";
import {MockSettlement} from "../src/mocks/MockSettlement.sol";

/// @title DeployCCV
/// @notice Deploy SymbioticCCV contracts on source and destination chains.
/// @dev Source deployment uses a local MockSettlement for now.
/// Destination deployment should use the real settlement deployed by DeployRelayInfra.
contract DeployCCV is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    function deploySource() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);
        bool useMockSettlement = vm.envOr("CCV_SOURCE_USE_MOCK_SETTLEMENT", true);
        address sourceSettlementAddress = vm.envOr("CCV_SOURCE_SETTLEMENT_ADDRESS", address(0));
        string memory sourceStorageLocation =
            vm.envOr("CCV_SOURCE_STORAGE_LOCATION", string("mock://symbiotic-ccv/source"));

        console.log("=== SymbioticCCV Source Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Use mock settlement:", useMockSettlement);

        vm.startBroadcast(deployer);

        address settlementToUse = sourceSettlementAddress;
        if (useMockSettlement) {
            settlementToUse = address(new MockSettlement());
        } else if (settlementToUse == address(0)) {
            revert("source settlement address required");
        }

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = sourceStorageLocation;

        SymbioticCCV ccv = new SymbioticCCV(settlementToUse, storageLocations);
        MockCCIPOnRamp onRamp = new MockCCIPOnRamp();
        vm.stopBroadcast();

        _saveSourceContracts(address(ccv), settlementToUse, address(onRamp));

        console.log("Source settlement:", settlementToUse);
        console.log("Source SymbioticCCV:", address(ccv));
        console.log("Source mock OnRamp:", address(onRamp));
        console.log("Saved to deploy-data/ccv_source_contracts.json");
    }

    function deployDest(address settlementAddr, uint64 sourceChainSelector) external {
        if (settlementAddr == address(0)) {
            revert("settlement address required");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);
        string memory destStorageLocation =
            vm.envOr("CCV_DEST_STORAGE_LOCATION", string("mock://symbiotic-ccv/destination"));

        console.log("=== SymbioticCCV Destination Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);

        vm.startBroadcast(deployer);

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = destStorageLocation;

        SymbioticCCV ccv = new SymbioticCCV(settlementAddr, storageLocations);
        MockCCIPOffRamp offRamp = new MockCCIPOffRamp(sourceChainSelector);
        vm.stopBroadcast();

        _saveDestContracts(address(ccv), settlementAddr, address(offRamp));

        console.log("Dest SymbioticCCV:", address(ccv));
        console.log("Dest mock OffRamp:", address(offRamp));
        console.log("Saved to deploy-data/ccv_dest_contracts.json");
    }

    function _saveSourceContracts(address ccv, address settlement, address onRamp) internal {
        string memory obj = "sourceCCV";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "ccv", ccv);
        vm.serializeAddress(obj, "settlement", settlement);
        string memory json = vm.serializeAddress(obj, "onRamp", onRamp);

        vm.writeJson(json, "deploy-data/ccv_source_contracts.json");
    }

    function _saveDestContracts(address ccv, address settlement, address offRamp) internal {
        string memory obj = "destCCV";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "ccv", ccv);
        vm.serializeAddress(obj, "settlement", settlement);
        string memory json = vm.serializeAddress(obj, "offRamp", offRamp);

        vm.writeJson(json, "deploy-data/ccv_dest_contracts.json");
    }
}
