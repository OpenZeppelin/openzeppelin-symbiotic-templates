// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";

import { LayerZeroLocalInfraStep } from "./DeployLayerZero.s.sol";
import { DvnStep } from "./DeployDVN.s.sol";
import { ExternalOAppConfigStep } from "./ConfigureExternalOApp.s.sol";
import { ExampleOAppStep } from "./DeployExampleOApp.s.sol";

contract DeployLayerZeroStack is Script, LayerZeroLocalInfraStep, DvnStep, ExternalOAppConfigStep, ExampleOAppStep {
    struct ChainConfig {
        uint256 sourceChainId;
        uint256 destChainId;
        uint32 sourceEid;
        uint32 destEid;
    }

    function deployLocal() external {
        ChainConfig memory config = ChainConfig({
            sourceChainId: vm.envOr("LZ_SOURCE_CHAIN_ID", uint256(31_337)),
            destChainId: vm.envOr("LZ_DEST_CHAIN_ID", uint256(31_338)),
            sourceEid: uint32(vm.envOr("LZ_SOURCE_EID", uint256(31_337))),
            destEid: uint32(vm.envOr("LZ_DEST_EID", uint256(31_338)))
        });

        (uint256 sourceFork, uint256 destFork) = _forks();

        vm.selectFork(sourceFork);
        _deploySourceInfra(config.sourceEid);

        vm.selectFork(destFork);
        _deployDestInfra(config.destEid);

        string memory relayJson = vm.readFile("deploy-data/relay_infra.json");
        address settlement = vm.parseJsonAddress(relayJson, ".settlement");
        address[3] memory operatorSubmitters = _operatorSubmitters();

        vm.selectFork(sourceFork);
        _deploySourceDvn(_readAddress("deploy-data/layerzero_source.json", ".sendUln"), config.sourceEid);
        _configureSourceUln(_readAddress("deploy-data/source_contracts.json", ".dvn"), config.destEid);

        vm.selectFork(destFork);
        _deployDestDvn(
            _readAddress("deploy-data/layerzero_dest.json", ".receiveUln"),
            settlement,
            config.destEid,
            operatorSubmitters
        );
        _configureDestUln(_readAddress("deploy-data/dest_contracts.json", ".dvn"), config.sourceEid);

        if (_oappEnabled()) {
            vm.selectFork(sourceFork);
            _deploySourceFromJson();

            vm.selectFork(destFork);
            _deployDestFromJson();

            vm.selectFork(sourceFork);
            _configurePeersFromJson();

            vm.selectFork(destFork);
            _configurePeersFromJson();
        }
    }

    function deployExternal() external {
        ChainConfig memory config = _configFromFiles();
        (uint256 sourceFork, uint256 destFork) = _forks();

        string memory relayJson = vm.readFile("deploy-data/relay_infra.json");
        address settlement = vm.parseJsonAddress(relayJson, ".settlement");
        address[3] memory operatorSubmitters = _operatorSubmitters();
        bool oappEnabled = _oappEnabled();

        vm.selectFork(sourceFork);
        _deploySourceDvn(_readAddress("deploy-data/layerzero_source.json", ".sendUln"), config.sourceEid);
        if (oappEnabled) {
            _deploySourceFromJson();
            _configureExternalSource(
                _readAddress("deploy-data/example_oapp_source.json", ".oapp"),
                _readAddress("deploy-data/source_contracts.json", ".dvn"),
                config.destEid
            );
        }

        vm.selectFork(destFork);
        _deployDestDvn(
            _readAddress("deploy-data/layerzero_dest.json", ".receiveUln"),
            settlement,
            config.destEid,
            operatorSubmitters
        );
        if (oappEnabled) {
            _deployDestFromJson();
            _configureExternalDest(
                _readAddress("deploy-data/example_oapp_dest.json", ".oapp"),
                _readAddress("deploy-data/dest_contracts.json", ".dvn"),
                config.sourceEid
            );

            vm.selectFork(sourceFork);
            _configurePeersFromJson();

            vm.selectFork(destFork);
            _configurePeersFromJson();
        }
    }

    function _oappEnabled() internal view returns (bool) {
        return vm.envOr("LAYERZERO_OAPP_ENABLED", true);
    }

    function _forks() internal returns (uint256 sourceFork, uint256 destFork) {
        string memory sourceRpc = _requiredEnv("SOURCE_RPC_URL", "SOURCE_RPC");
        string memory destRpc = _requiredEnv("DEST_RPC_URL", "DEST_RPC");
        sourceFork = vm.createSelectFork(sourceRpc);
        destFork = vm.createFork(destRpc);
    }

    function _configFromFiles() internal view returns (ChainConfig memory) {
        string memory sourceJson = vm.readFile("deploy-data/layerzero_source.json");
        string memory destJson = vm.readFile("deploy-data/layerzero_dest.json");
        return ChainConfig({
            sourceChainId: vm.parseJsonUint(sourceJson, ".chainId"),
            destChainId: vm.parseJsonUint(destJson, ".chainId"),
            sourceEid: uint32(vm.parseJsonUint(sourceJson, ".eid")),
            destEid: uint32(vm.parseJsonUint(destJson, ".eid"))
        });
    }

    function _requiredEnv(string memory primary, string memory fallbackName) internal view returns (string memory) {
        string memory value = vm.envOr(primary, string(""));
        if (bytes(value).length == 0) {
            value = vm.envOr(fallbackName, string(""));
        }
        require(bytes(value).length > 0, string(abi.encodePacked(primary, " or ", fallbackName, " must be set")));
        return value;
    }

    function _readAddress(string memory path, string memory key) internal view returns (address) {
        string memory json = vm.readFile(path);
        return vm.parseJsonAddress(json, key);
    }

    function _operatorSubmitters() internal view returns (address[3] memory submitters) {
        for (uint256 i = 0; i < submitters.length; i++) {
            string memory envName = string(abi.encodePacked("OPERATOR_", vm.toString(i + 1), "_PRIVATE_KEY"));
            submitters[i] = vm.addr(vm.envUint(envName));
        }
    }
}
