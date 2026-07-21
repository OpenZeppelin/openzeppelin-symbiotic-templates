// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";

import { ExampleOApp } from "../../src/layerzero/ExampleOApp.sol";

abstract contract ExampleOAppStep is Script {
    function _deploySourceFromJson() internal {
        address deployer = _oappDeployerAddress();
        string memory json = vm.readFile("deploy-data/layerzero/layerzero_source.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== ExampleOApp Source Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        _startOAppBroadcast();

        ExampleOApp oapp = new ExampleOApp(endpoint, deployer);
        console.log("ExampleOApp (Source):", address(oapp));

        vm.stopBroadcast();

        _saveSourceContract(address(oapp), endpoint);
    }

    function _deployDestFromJson() internal {
        address deployer = _oappDeployerAddress();
        string memory json = vm.readFile("deploy-data/layerzero/layerzero_dest.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== ExampleOApp Destination Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        _startOAppBroadcast();

        ExampleOApp oapp = new ExampleOApp(endpoint, deployer);
        console.log("ExampleOApp (Dest):", address(oapp));

        vm.stopBroadcast();

        _saveDestContract(address(oapp), endpoint);
    }

    function _configurePeersFromJson() internal {
        string memory srcJson = vm.readFile("deploy-data/layerzero/example_oapp_source.json");
        string memory dstJson = vm.readFile("deploy-data/layerzero/example_oapp_dest.json");
        address srcOApp = vm.parseJsonAddress(srcJson, ".oapp");
        address dstOApp = vm.parseJsonAddress(dstJson, ".oapp");

        string memory lzSrcJson = vm.readFile("deploy-data/layerzero/layerzero_source.json");
        string memory lzDstJson = vm.readFile("deploy-data/layerzero/layerzero_dest.json");
        uint256 sourceChainId = vm.parseJsonUint(lzSrcJson, ".chainId");
        uint256 destChainId = vm.parseJsonUint(lzDstJson, ".chainId");
        uint256 sourceEidRaw = vm.parseJsonUint(lzSrcJson, ".eid");
        uint256 destEidRaw = vm.parseJsonUint(lzDstJson, ".eid");
        require(sourceEidRaw <= type(uint32).max, "source eid exceeds uint32");
        require(destEidRaw <= type(uint32).max, "dest eid exceeds uint32");

        _configurePeers(srcOApp, dstOApp, sourceChainId, destChainId, uint32(sourceEidRaw), uint32(destEidRaw));
    }

    function _configurePeers(
        address srcOApp,
        address dstOApp,
        uint256 sourceChainId,
        uint256 destChainId,
        uint32 sourceEid,
        uint32 destEid
    )
        internal
    {
        address deployer = _oappDeployerAddress();

        console.log("=== Configuring OApp Peers ===");
        console.log("Chain ID:", block.chainid);
        console.log("Source chain ID:", sourceChainId);
        console.log("Source EID:", sourceEid);
        console.log("Dest chain ID:", destChainId);
        console.log("Dest EID:", destEid);
        console.log("Source OApp:", srcOApp);
        console.log("Dest OApp:", dstOApp);
        console.log("Deployer:", deployer);

        _startOAppBroadcast();

        if (block.chainid == sourceChainId) {
            ExampleOApp oapp = ExampleOApp(srcOApp);
            bytes32 dstPeer = bytes32(uint256(uint160(dstOApp)));
            oapp.setPeer(destEid, dstPeer);
            console.log("Source OApp peer set for EID", destEid);
            console.log("  Peer:", dstOApp);
        } else if (block.chainid == destChainId) {
            ExampleOApp oapp = ExampleOApp(dstOApp);
            bytes32 srcPeer = bytes32(uint256(uint160(srcOApp)));
            oapp.setPeer(sourceEid, srcPeer);
            console.log("Dest OApp peer set for EID", sourceEid);
            console.log("  Peer:", srcOApp);
        } else {
            revert("Unknown chain ID - expected source/destination chain ID");
        }

        vm.stopBroadcast();
    }

    function _saveSourceContract(address oapp, address endpoint) internal {
        string memory obj = "sourceExampleOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "oapp", oapp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.createDir("deploy-data/layerzero", true);
        vm.writeJson(json, "deploy-data/layerzero/example_oapp_source.json");
        console.log("Saved to deploy-data/layerzero/example_oapp_source.json");
    }

    function _saveDestContract(address oapp, address endpoint) internal {
        string memory obj = "destExampleOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "oapp", oapp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.createDir("deploy-data/layerzero", true);
        vm.writeJson(json, "deploy-data/layerzero/example_oapp_dest.json");
        console.log("Saved to deploy-data/layerzero/example_oapp_dest.json");
    }

    function _oappDeployerAddress() internal view returns (address) {
        if (vm.envExists("DEPLOYER_ADDRESS")) {
            return vm.envAddress("DEPLOYER_ADDRESS");
        }
        if (vm.envExists("PRIVATE_KEY")) {
            return vm.addr(vm.envUint("PRIVATE_KEY"));
        }
        return msg.sender;
    }

    function _startOAppBroadcast() internal {
        if (vm.envExists("PRIVATE_KEY")) {
            vm.startBroadcast(vm.envUint("PRIVATE_KEY"));
        } else {
            vm.startBroadcast();
        }
    }
}

contract SendExampleMessage is Script {
    function run(address oappAddress, uint32 dstEid, string calldata message) external {
        address sender = msg.sender;

        console.log("=== Sending Example Message ===");
        console.log("ExampleOApp:", oappAddress);
        console.log("Destination EID:", dstEid);
        console.log("Message:", message);
        console.log("Sender:", sender);

        ExampleOApp oapp = ExampleOApp(oappAddress);
        bytes memory options = oapp.buildOptions(200_000);
        uint256 fee = oapp.quote(dstEid, message, options, false).nativeFee;
        console.log("Fee (native):", fee);

        vm.startBroadcast();
        oapp.send{ value: fee }(dstEid, message, options);
        vm.stopBroadcast();

        console.log("");
        console.log("=== Message Sent ===");
        console.log("Messages sent total:", oapp.messagesSent());
    }
}
