// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {SymbioticVerifier} from "../src/ccv/SymbioticVerifier.sol";

/// @title ConfigureCCV
/// @notice Configures a remote chain on a deployed SymbioticVerifier contract.
contract ConfigureCCV is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @dev Destination gas declared to the OnRamp for SymbioticVerifier.verifyMessage,
    /// buffered above the ~312k worst case observed on Sepolia. Real cost scales
    /// with validator-set size; revisit when the production valset grows.
    uint256 constant DEFAULT_GAS_FOR_VERIFICATION = 400_000;

    function run(address verifierAddress) external {
        if (verifierAddress == address(0)) {
            revert("verifier address required");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);
        uint64 remoteChainSelector = uint64(vm.envUint("CCV_REMOTE_CHAIN_SELECTOR"));
        address router = vm.envAddress("CCV_ROUTER_ADDRESS");
        bool allowlistEnabled = vm.envOr("CCV_ALLOWLIST_ENABLED", false);
        uint16 feeUSDCents = uint16(vm.envOr("CCV_FEE_USD_CENTS", uint256(0)));
        uint32 gasForVerification =
            uint32(vm.envOr("CCV_GAS_FOR_VERIFICATION", DEFAULT_GAS_FOR_VERIFICATION));
        uint16 payloadSizeBytes = uint16(vm.envOr("CCV_PAYLOAD_SIZE_BYTES", uint256(0)));

        console.log("=== Configure SymbioticVerifier ===");
        console.log("Chain ID:", block.chainid);
        console.log("Verifier:", verifierAddress);
        console.log("Remote selector:", remoteChainSelector);
        console.log("Router:", router);
        console.log("Allowlist enabled:", allowlistEnabled);

        BaseVerifier.RemoteChainConfigArgs[] memory updates = new BaseVerifier.RemoteChainConfigArgs[](1);
        updates[0] = BaseVerifier.RemoteChainConfigArgs({
            router: IRouter(router),
            remoteChainSelector: remoteChainSelector,
            allowlistEnabled: allowlistEnabled,
            feeUSDCents: feeUSDCents,
            gasForVerification: gasForVerification,
            payloadSizeBytes: payloadSizeBytes
        });

        vm.startBroadcast(deployer);
        SymbioticVerifier(verifierAddress).applyRemoteChainConfigUpdates(updates);
        vm.stopBroadcast();

        console.log("Configured.");
    }
}
