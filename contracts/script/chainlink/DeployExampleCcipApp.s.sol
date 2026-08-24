// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {Client} from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

import {ExampleCcipApp} from "../../src/chainlink/ExampleCcipApp.sol";

/// @notice Deploys ExampleCcipApp (both chains) and wires the cross-chain peer
/// relationships via setRemoteApp.
contract DeployExampleCcipApp is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Deploy ExampleCcipApp.
    /// @param router CCIP Router on this chain.
    /// @param ccv Local VersionedVerifierResolver address.
    /// @param executor Executor encoded into every message this app sends. Use
    /// Client.NO_EXECUTION_ADDRESS (manual execution: nothing charged, our operator
    /// self-executes). Any other IExecutor contract is PAID the destination-execution
    /// portion of every send and must be able to disburse it. It does not affect
    /// inbound messages, but the destination app's own replies use it — set the
    /// manual-execution sentinel on both apps when both directions self-execute.
    /// @param outputPath Relative path inside contracts/ for the deploy-data JSON.
    function deployApp(address router, address ccv, address executor, string calldata outputPath) external {
        if (router == address(0)) revert("router required");
        if (ccv == address(0)) revert("ccv required");
        if (executor == address(0)) {
            revert("executor required (use Client.NO_EXECUTION_ADDRESS for manual/operator execution)");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== ExampleCcipApp Deployment ===");
        console.log("Chain ID:", block.chainid);
        console.log("Router:", router);
        console.log("CCV:", ccv);
        console.log("Executor:", executor);

        vm.startBroadcast(deployer);
        ExampleCcipApp app = new ExampleCcipApp(router, ccv, executor);
        vm.stopBroadcast();

        string memory obj = "exampleCcipApp";
        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "app", address(app));
        vm.serializeAddress(obj, "router", router);
        vm.serializeAddress(obj, "ccv", ccv);
        string memory json = vm.serializeAddress(obj, "executor", executor);
        vm.createDir("deploy-data/chainlink", true);
        vm.writeJson(json, outputPath);

        console.log("ExampleCcipApp:", address(app));
        console.log("Saved to", outputPath);
    }

    /// @notice Wire setRemoteApp(remoteChainSelector, remoteApp) on a deployed ExampleCcipApp.
    function setRemote(address app, uint64 remoteSelector, address remoteApp) external {
        if (app == address(0)) revert("app required");
        if (remoteSelector == 0) revert("remote selector required");
        if (remoteApp == address(0)) revert("remote app address required");

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== ExampleCcipApp setRemoteApp ===");
        console.log("App:", app);
        console.log("Remote selector:", remoteSelector);
        console.log("Remote app:", remoteApp);

        vm.startBroadcast(deployer);
        ExampleCcipApp(payable(app)).setRemoteApp(remoteSelector, remoteApp);
        vm.stopBroadcast();
    }
}
