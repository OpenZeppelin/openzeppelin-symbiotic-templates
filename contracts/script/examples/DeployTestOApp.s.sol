// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {TestOApp} from "../../src/examples/TestOApp.sol";

abstract contract TestOAppStep is Script {
    function _deploySourceFromJson() internal {
        address deployer = _oappDeployerAddress();
        string memory json = vm.readFile("deploy-data/layerzero_source.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== TestOApp Source Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        _startOAppBroadcast();

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Source):", address(testOApp));

        vm.stopBroadcast();

        _saveSourceContract(address(testOApp), endpoint);
    }

    function _deployDestFromJson() internal {
        address deployer = _oappDeployerAddress();
        string memory json = vm.readFile("deploy-data/layerzero_dest.json");
        address endpoint = vm.parseJsonAddress(json, ".endpoint");

        console.log("=== TestOApp Destination Chain Deployment (from JSON) ===");
        console.log("Chain ID:", block.chainid);
        console.log("Endpoint (from JSON):", endpoint);
        console.log("Deployer:", deployer);

        _startOAppBroadcast();

        TestOApp testOApp = new TestOApp(endpoint, deployer);
        console.log("TestOApp (Dest):", address(testOApp));

        vm.stopBroadcast();

        _saveDestContract(address(testOApp), endpoint);
    }

    function _configurePeersFromJson() internal {
        string memory srcJson = vm.readFile("deploy-data/testoapp_source.json");
        string memory dstJson = vm.readFile("deploy-data/testoapp_dest.json");
        address srcOApp = vm.parseJsonAddress(srcJson, ".testOApp");
        address dstOApp = vm.parseJsonAddress(dstJson, ".testOApp");

        string memory lzSrcJson = vm.readFile("deploy-data/layerzero_source.json");
        string memory lzDstJson = vm.readFile("deploy-data/layerzero_dest.json");
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
    ) internal {
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
            TestOApp oapp = TestOApp(srcOApp);
            bytes32 dstPeer = bytes32(uint256(uint160(dstOApp)));
            oapp.setPeer(destEid, dstPeer);
            console.log("Source OApp peer set for EID", destEid);
            console.log("  Peer:", dstOApp);
        } else if (block.chainid == destChainId) {
            TestOApp oapp = TestOApp(dstOApp);
            bytes32 srcPeer = bytes32(uint256(uint160(srcOApp)));
            oapp.setPeer(sourceEid, srcPeer);
            console.log("Dest OApp peer set for EID", sourceEid);
            console.log("  Peer:", srcOApp);
        } else {
            revert("Unknown chain ID - expected source/destination chain ID");
        }

        vm.stopBroadcast();
    }

    function _saveSourceContract(address testOApp, address endpoint) internal {
        string memory obj = "sourceTestOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "testOApp", testOApp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.writeJson(json, "deploy-data/testoapp_source.json");
        console.log("Saved to deploy-data/testoapp_source.json");
    }

    function _saveDestContract(address testOApp, address endpoint) internal {
        string memory obj = "destTestOApp";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "testOApp", testOApp);
        string memory json = vm.serializeAddress(obj, "endpoint", endpoint);

        vm.writeJson(json, "deploy-data/testoapp_dest.json");
        console.log("Saved to deploy-data/testoapp_dest.json");
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

contract SendTestMessage is Script {
    function run(address testOApp, uint32 dstEid, string calldata message) external {
        address sender = msg.sender;

        console.log("=== Sending Test Message ===");
        console.log("TestOApp:", testOApp);
        console.log("Destination EID:", dstEid);
        console.log("Message:", message);
        console.log("Sender:", sender);

        TestOApp oapp = TestOApp(testOApp);
        bytes memory options = oapp.buildOptions(200_000);
        uint256 fee = oapp.quote(dstEid, message, options, false).nativeFee;
        console.log("Fee (native):", fee);

        vm.startBroadcast();
        oapp.send{value: fee}(dstEid, message, options);
        vm.stopBroadcast();

        console.log("");
        console.log("=== Message Sent ===");
        console.log("Messages sent total:", oapp.messagesSent());
    }
}
