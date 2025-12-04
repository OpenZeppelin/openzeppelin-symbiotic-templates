// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {INetworkManager} from "@symbioticfi/relay-contracts/interfaces/modules/base/INetworkManager.sol";
import {ISettlement} from "@symbioticfi/relay-contracts/interfaces/modules/settlement/ISettlement.sol";
import {IOzEIP712} from "@symbioticfi/relay-contracts/interfaces/modules/base/IOzEIP712.sol";
import {SigVerifierBlsBn254Simple} from
    "@symbioticfi/relay-contracts/modules/settlement/sig-verifiers/SigVerifierBlsBn254Simple.sol";

import {Settlement} from "../src/symbiotic/Settlement.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @title DestinationChainDeploy
/// @notice Phase 2: Deploy Settlement + DVN on destination chain
/// @dev Settlement verifies BLS proofs, DVN calls ReceiveUln302
contract DestinationChainDeploy is Script {
    struct CrossChainAddress {
        address addr;
        uint64 chainId;
    }

    // NOTE: Fields MUST be in alphabetical order for Foundry's parseJson/abi.decode
    struct DestChainContracts {
        CrossChainAddress dvn;
        CrossChainAddress settlement;
    }

    uint256 internal constant DVN_BASE_FEE = 0.001 ether;

    address internal deployer;
    address internal networkAddress;

    Settlement internal settlement;
    SymbioticLayerZeroDVN internal dvn;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266);
    }

    function getNetworkAddress() internal view returns (address) {
        // Network address from source chain deployment
        // In production, this would be read from source_chain_contracts.json
        return vm.envOr("NETWORK_ADDRESS", address(0));
    }

    function run() public {
        deployer = getDeployerAddress();
        networkAddress = getNetworkAddress();

        console.log("=== Phase 2: Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Network Address (from source):", networkAddress);

        if (networkAddress == address(0)) {
            console.log("");
            console.log("WARNING: NETWORK_ADDRESS not set. Using mock network address.");
            console.log("For production, load from source_chain_contracts.json");
            networkAddress = address(0x1234); // Placeholder
        }

        setupSettlement();
        setupDVN();

        logAndDumpDestChainContracts();

        console.log("");
        console.log("=== Destination Chain Deployment Complete ===");
        console.log("Next: Run DriverDeploy.s.sol on source chain");
    }

    function setupSettlement() public returns (address) {
        vm.startBroadcast(deployer);

        settlement = new Settlement{salt: "Settlement"}();

        address verifier = address(new SigVerifierBlsBn254Simple());

        settlement.initialize(
            ISettlement.SettlementInitParams({
                networkManagerInitParams: INetworkManager.NetworkManagerInitParams({
                    network: networkAddress,
                    subnetworkId: 0
                }),
                ozEip712InitParams: IOzEIP712.OzEIP712InitParams({name: "Settlement", version: "1"}),
                sigVerifier: verifier
            }),
            deployer
        );

        vm.stopBroadcast();

        console.log("Settlement:", address(settlement));
        console.log("SigVerifier (BLS-BN254):", verifier);
        return address(settlement);
    }

    function setupDVN() public returns (address) {
        vm.startBroadcast(deployer);

        // On destination chain, DVN uses Settlement for proof verification
        dvn = new SymbioticLayerZeroDVN(
            address(settlement),
            DVN_BASE_FEE
        );

        vm.stopBroadcast();

        console.log("DVN (Destination):", address(dvn));
        return address(dvn);
    }

    function logAndDumpDestChainContracts() public {
        string memory obj = "destChainContracts";

        vm.serializeUint("settlement", "chainId", block.chainid);
        string memory settlementData = vm.serializeAddress("settlement", "addr", address(settlement));
        vm.serializeString(obj, "settlement", settlementData);

        vm.serializeUint("dvn", "chainId", block.chainid);
        string memory dvnData = vm.serializeAddress("dvn", "addr", address(dvn));
        string memory finalJson = vm.serializeString(obj, "dvn", dvnData);

        vm.writeJson(finalJson, "devnet/deploy-data/dest_chain_contracts.json");
        console.log("Contracts saved to devnet/deploy-data/dest_chain_contracts.json");
    }

    /// @notice Configure DVN with ReceiveUln302 address (call after LayerZero deployment)
    function configureReceiveUln(address receiveUln) public {
        vm.startBroadcast(deployer);
        dvn.setReceiveUln(receiveUln);
        vm.stopBroadcast();

        console.log("DVN configured with ReceiveUln:", receiveUln);
    }
}
