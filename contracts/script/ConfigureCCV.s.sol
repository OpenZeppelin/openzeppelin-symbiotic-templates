// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {SymbioticCCV} from "../src/ccv/SymbioticCCV.sol";

/// @title ConfigureCCV
/// @notice Configures remote-chain caller permissions on a deployed SymbioticCCV contract.
contract ConfigureCCV is Script {
    address constant DEFAULT_DEPLOYER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    /// @dev Destination gas declared to the OnRamp for SymbioticCCV.verifyMessage,
    /// buffered above the ~312k worst case observed on Sepolia. Real cost scales
    /// with validator-set size; revisit when the production valset grows.
    uint256 constant DEFAULT_GAS_FOR_VERIFICATION = 400_000;

    function run(address ccvAddress) external {
        if (ccvAddress == address(0)) {
            revert("ccv address required");
        }

        address deployer = vm.envOr("DEPLOYER_ADDRESS", DEFAULT_DEPLOYER);
        uint64 remoteChainSelector = uint64(vm.envUint("CCV_REMOTE_CHAIN_SELECTOR"));
        address onRamp = vm.envAddress("CCV_ONRAMP_ADDRESS");
        address offRamp = vm.envAddress("CCV_OFFRAMP_ADDRESS");
        bool allowlistEnabled = vm.envOr("CCV_ALLOWLIST_ENABLED", false);
        uint16 feeUSDCents = uint16(vm.envOr("CCV_FEE_USD_CENTS", uint256(0)));
        uint32 gasForVerification =
            uint32(vm.envOr("CCV_GAS_FOR_VERIFICATION", DEFAULT_GAS_FOR_VERIFICATION));
        uint32 payloadSizeBytes = uint32(vm.envOr("CCV_PAYLOAD_SIZE_BYTES", uint256(0)));

        console.log("=== Configure SymbioticCCV ===");
        console.log("Chain ID:", block.chainid);
        console.log("CCV:", ccvAddress);
        console.log("Remote selector:", remoteChainSelector);
        console.log("OnRamp:", onRamp);
        console.log("OffRamp:", offRamp);
        console.log("Allowlist enabled:", allowlistEnabled);

        SymbioticCCV.RemoteChainConfigArgs[] memory updates = new SymbioticCCV.RemoteChainConfigArgs[](1);
        updates[0] = SymbioticCCV.RemoteChainConfigArgs({
            remoteChainSelector: remoteChainSelector,
            onRamp: onRamp,
            offRamp: offRamp,
            allowlistEnabled: allowlistEnabled,
            feeUSDCents: feeUSDCents,
            gasForVerification: gasForVerification,
            payloadSizeBytes: payloadSizeBytes
        });

        vm.startBroadcast(deployer);
        SymbioticCCV(ccvAddress).applyRemoteChainConfigUpdates(updates);
        vm.stopBroadcast();

        console.log("Configured.");
    }
}
