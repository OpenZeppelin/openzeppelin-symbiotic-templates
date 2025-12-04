// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script, console} from "forge-std/Script.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @notice Simple deployment script for SymbioticLayerZeroDVN
/// @dev For devnet, uses a mock settlement. For production, pass real settlement address.
contract DeployDVN is Script {
    uint256 constant BASE_FEE = 0.001 ether;

    function run() external {
        address settlementAddr = vm.envOr("SETTLEMENT_ADDRESS", address(0));

        vm.startBroadcast();

        // If no settlement provided, deploy a mock
        if (settlementAddr == address(0)) {
            MockSettlement mock = new MockSettlement();
            settlementAddr = address(mock);
            console.log("Deployed MockSettlement:", settlementAddr);
        }

        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(settlementAddr, BASE_FEE);
        console.log("Deployed SymbioticLayerZeroDVN:", address(dvn));
        console.log("  Settlement:", settlementAddr);
        console.log("  Base Fee:", BASE_FEE);

        vm.stopBroadcast();

        // Write addresses to JSON
        string memory json = string.concat(
            '{"dvn":"',
            vm.toString(address(dvn)),
            '","settlement":"',
            vm.toString(settlementAddr),
            '","baseFee":"',
            vm.toString(BASE_FEE),
            '"}'
        );
        vm.writeFile("devnet/deploy-data/dvn.json", json);
        console.log("Wrote addresses to devnet/deploy-data/dvn.json");
    }
}

/// @notice Mock settlement for local testing
contract MockSettlement {
    function getCaptureTimestampFromValSetHeaderAt(uint48) external pure returns (uint48) {
        return 0;
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure returns (uint8) {
        return 15;
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure returns (uint256) {
        return 6667;
    }

    function verifyQuorumSigAt(
        bytes memory,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external pure returns (bool) {
        return true; // Always valid for testing
    }
}
