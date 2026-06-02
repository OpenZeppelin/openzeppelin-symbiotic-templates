// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";
import {MockCCIPOffRamp} from "../src/mocks/MockCCIPOffRamp.sol";
import {MockCCIPOnRamp} from "../src/mocks/MockCCIPOnRamp.sol";
import {NoOpSettlement} from "../src/mocks/NoOpSettlement.sol";

/// @title DeployCCV
/// @notice Deploy SymbioticCCV contracts on source and destination chains.
///
/// Two modes:
///   - `deploySource(settlement, destSelector)` / `deployDest(settlement, sourceSelector)`:
///     local devnet flow that also deploys mock OnRamp/OffRamp pairs so the operator
///     stack has something to listen to and submit against.
///   - `deploySourceCcvOnly(settlement, onRamp, offRamp)` /
///     `deployDestCcvOnly(settlement, onRamp, offRamp)`:
///     testnet flow when the real CCIP staging contracts are present as predeploys.
///     Skips the mock contract deploys and only writes the CCV address.
///   - `deployNoOpSettlement()`: deploys a NoOpSettlement, for source-side
///     dest-only Symbiotic deployments where we don't run a full relay infra.
contract DeployCCV is Script {
    string constant SOURCE_STORAGE_LOCATION = "mock://symbiotic-ccv/source";
    string constant DEST_STORAGE_LOCATION = "mock://symbiotic-ccv/destination";

    function deploySource(address settlementAddr, uint64 destChainSelector) external {
        if (settlementAddr == address(0)) {
            revert("settlement address required");
        }
        if (destChainSelector == 0) {
            revert("dest chain selector required");
        }

        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        console.log("=== SymbioticCCV Source Deployment (with mocks) ===");
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

        console.log("Source SymbioticCCV:", address(ccv));
        console.log("Source mock OnRamp:", address(onRamp));
        console.log("Source mock OffRamp:", address(offRamp));
        console.log("Saved to deploy-data/ccv_source_contracts.json");
    }

    function deployDest(address settlementAddr, uint64 sourceChainSelector) external {
        if (settlementAddr == address(0)) {
            revert("settlement address required");
        }

        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        console.log("=== SymbioticCCV Destination Deployment (with mocks) ===");
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

    /// @notice Deploy SymbioticCCV on source against real CCIP predeploys.
    /// @param settlementAddr Source-side settlement (typically NoOpSettlement when running dest-only Symbiotic).
    /// @param onRampAddr Real Chainlink OnRamp on this chain (from chain predeploys).
    /// @param offRampAddr Real Chainlink OffRamp on this chain (from chain predeploys). Recorded but unused on source.
    function deploySourceCcvOnly(address settlementAddr, address onRampAddr, address offRampAddr) external {
        if (settlementAddr == address(0)) revert("settlement address required");
        if (onRampAddr == address(0)) revert("onRamp address required");
        if (offRampAddr == address(0)) revert("offRamp address required");

        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        console.log("=== SymbioticCCV Source Deployment (real CCIP) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);
        console.log("Real OnRamp:", onRampAddr);

        vm.startBroadcast(deployer);

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = SOURCE_STORAGE_LOCATION;
        SymbioticCCV ccv = new SymbioticCCV(settlementAddr, storageLocations);

        vm.stopBroadcast();

        _saveSourceContracts(address(ccv), settlementAddr, onRampAddr, offRampAddr);
        console.log("Source SymbioticCCV:", address(ccv));
    }

    function deployDestCcvOnly(address settlementAddr, address onRampAddr, address offRampAddr) external {
        if (settlementAddr == address(0)) revert("settlement address required");
        if (onRampAddr == address(0)) revert("onRamp address required");
        if (offRampAddr == address(0)) revert("offRamp address required");

        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        console.log("=== SymbioticCCV Destination Deployment (real CCIP) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Settlement:", settlementAddr);
        console.log("Real OffRamp:", offRampAddr);

        vm.startBroadcast(deployer);

        string[] memory storageLocations = new string[](1);
        storageLocations[0] = DEST_STORAGE_LOCATION;
        SymbioticCCV ccv = new SymbioticCCV(settlementAddr, storageLocations);

        vm.stopBroadcast();

        _saveDestContracts(address(ccv), settlementAddr, onRampAddr, offRampAddr);
        console.log("Dest SymbioticCCV:", address(ccv));
    }

    /// @notice Deploy a NoOpSettlement contract. Used as the source-side Settlement when
    /// running dest-only Symbiotic on chains that don't run the relay infrastructure.
    function deployNoOpSettlement() external {
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        console.log("=== NoOpSettlement Deployment ===");
        console.log("Chain ID:", block.chainid);

        vm.startBroadcast(deployer);
        NoOpSettlement noOp = new NoOpSettlement();
        vm.stopBroadcast();

        string memory obj = "noOpSettlement";
        vm.serializeUint(obj, "chainId", block.chainid);
        string memory json = vm.serializeAddress(obj, "settlement", address(noOp));
        vm.writeJson(json, "deploy-data/noop_settlement.json");

        console.log("NoOpSettlement:", address(noOp));
        console.log("Saved to deploy-data/noop_settlement.json");
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
