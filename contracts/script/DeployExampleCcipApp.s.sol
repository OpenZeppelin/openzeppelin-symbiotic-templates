// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {ExampleCcipApp} from "../src/examples/ExampleCcipApp.sol";
import {NoOpExecutor} from "../src/examples/NoOpExecutor.sol";

/// @notice Deploys NoOpExecutor (source only) and ExampleCcipApp (both chains),
/// and wires the cross-chain peer relationships via setRemoteApp.
contract DeployExampleCcipApp is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @notice Deploy NoOpExecutor. Used as the source-side executor stub so
    /// CCIP's FeeQuoter has an IExecutor to call during fee quoting.
    function deployExecutor() external {
        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);

        console.log("=== NoOpExecutor Deployment ===");
        console.log("Chain ID:", block.chainid);

        vm.startBroadcast(deployer);
        NoOpExecutor exec = new NoOpExecutor();
        vm.stopBroadcast();

        string memory obj = "noOpExecutor";
        vm.serializeUint(obj, "chainId", block.chainid);
        string memory json = vm.serializeAddress(obj, "executor", address(exec));
        vm.writeJson(json, "deploy-data/noop_executor.json");

        console.log("NoOpExecutor:", address(exec));
    }

    /// @notice Deploy ExampleCcipApp.
    /// @param router CCIP Router on this chain.
    /// @param ccv Local VersionedVerifierResolver address.
    /// @param executor Source-side: NoOpExecutor address. Destination-side: any (unused).
    /// @param outputPath Relative path inside contracts/ for the deploy-data JSON.
    function deployApp(address router, address ccv, address executor, string calldata outputPath) external {
        if (router == address(0)) revert("router required");
        if (ccv == address(0)) revert("ccv required");
        if (executor == address(0)) revert("executor required (use NoOpExecutor on source)");

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
