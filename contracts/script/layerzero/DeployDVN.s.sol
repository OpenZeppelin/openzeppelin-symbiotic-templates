// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticLayerZeroDVN} from "../../src/layerzero/SymbioticLayerZeroDVN.sol";

abstract contract DvnStep is Script {
    uint256 internal constant BASE_FEE = 0;

    function _deploySourceDvn(address sendUlnAddr, uint32 sourceEid) internal {
        address deployer = _dvnDeployerAddress();

        console.log("=== DVN Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("SendUln302Mock:", sendUlnAddr);

        _startDvnBroadcast();

        SymbioticLayerZeroDVN dvn =
            new SymbioticLayerZeroDVN(address(0), sendUlnAddr, address(0), sourceEid, BASE_FEE, 0);
        console.log("DVN (Source):", address(dvn));

        vm.stopBroadcast();

        _saveSourceContracts(address(dvn), sendUlnAddr);
    }

    function _deployDestDvn(
        address receiveUlnAddr,
        address settlementAddr,
        uint32 destEid,
        address[3] memory operatorSubmitters
    ) internal {
        address deployer = _dvnDeployerAddress();
        address submitter = vm.envOr("SUBMITTER_ADDRESS", deployer);

        console.log("=== DVN Destination Chain Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("ReceiveUln302Mock:", receiveUlnAddr);
        console.log("Settlement:", settlementAddr);
        console.log("Submitter:", submitter);

        // The epoch validity ceiling must not exceed the Symbiotic slashing window: a
        // proof must only verify while the attesting stake is still slashable.
        uint256 maxEpochValidity = vm.envOr("SLASHING_WINDOW", uint256(0));
        require(
            maxEpochValidity != 0,
            "SLASHING_WINDOW (seconds) is required: the epoch validity ceiling must match the deployment's Symbiotic slashing window"
        );
        console.log("Max epoch validity:", maxEpochValidity);

        _startDvnBroadcast();

        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            settlementAddr, address(0), receiveUlnAddr, destEid, BASE_FEE, maxEpochValidity
        );
        console.log("DVN (Dest):", address(dvn));

        dvn.addSubmitter(submitter);
        console.log("Submitter added:", submitter);

        for (uint256 i = 0; i < operatorSubmitters.length; i++) {
            if (operatorSubmitters[i] == address(0) || operatorSubmitters[i] == submitter) {
                continue;
            }
            dvn.addSubmitter(operatorSubmitters[i]);
            console.log("Operator submitter added:", operatorSubmitters[i]);
        }

        // Add relayer signer addresses as submitters (separate from operator keys on non-local)
        string[3] memory signerEnvs = ["SIGNER_1_ADDRESS", "SIGNER_2_ADDRESS", "SIGNER_3_ADDRESS"];
        for (uint256 i = 0; i < signerEnvs.length; i++) {
            address signerAddr = vm.envOr(signerEnvs[i], address(0));
            if (signerAddr == address(0) || signerAddr == submitter) {
                continue;
            }
            dvn.addSubmitter(signerAddr);
            console.log("Relayer signer submitter added:", signerAddr);
        }

        vm.stopBroadcast();

        _saveDestContracts(address(dvn), receiveUlnAddr, settlementAddr);
    }

    function _saveSourceContracts(address dvn, address sendUln) internal {
        string memory obj = "sourceContracts";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "dvn", dvn);
        string memory json = vm.serializeAddress(obj, "sendUln", sendUln);

        vm.createDir("deploy-data/layerzero", true);
        vm.writeJson(json, "deploy-data/layerzero/source_contracts.json");
        console.log("Saved to deploy-data/layerzero/source_contracts.json");
    }

    function _saveDestContracts(address dvn, address receiveUln, address settlement) internal {
        string memory obj = "destContracts";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "dvn", dvn);
        vm.serializeAddress(obj, "receiveUln", receiveUln);
        string memory json = vm.serializeAddress(obj, "settlement", settlement);

        vm.createDir("deploy-data/layerzero", true);
        vm.writeJson(json, "deploy-data/layerzero/dest_contracts.json");
        console.log("Saved to deploy-data/layerzero/dest_contracts.json");
    }

    function _dvnDeployerAddress() internal view returns (address) {
        if (vm.envExists("DEPLOYER_ADDRESS")) {
            return vm.envAddress("DEPLOYER_ADDRESS");
        }
        if (vm.envExists("PRIVATE_KEY")) {
            return vm.addr(vm.envUint("PRIVATE_KEY"));
        }
        return msg.sender;
    }

    function _startDvnBroadcast() internal {
        if (vm.envExists("PRIVATE_KEY")) {
            vm.startBroadcast(vm.envUint("PRIVATE_KEY"));
        } else {
            vm.startBroadcast();
        }
    }
}
