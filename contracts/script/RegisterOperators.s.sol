// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {Vm} from "forge-std/Vm.sol";

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

import {OperatorRegistry} from "@symbioticfi/core/src/contracts/OperatorRegistry.sol";
import {OptInService} from "@symbioticfi/core/src/contracts/service/OptInService.sol";
import {IVault} from "@symbioticfi/core/src/interfaces/vault/IVault.sol";

import {IKeyRegistry} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";
import {IOzEIP712} from "@symbioticfi/relay-contracts/interfaces/modules/base/IOzEIP712.sol";
import {KeyTags} from "@symbioticfi/relay-contracts/libraries/utils/KeyTags.sol";
import {KeyBlsBn254, BN254} from "@symbioticfi/relay-contracts/libraries/keys/KeyBlsBn254.sol";
import {KEY_TYPE_BLS_BN254} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";

import {KeyRegistry} from "../src/symbiotic/KeyRegistry.sol";
import {VotingPowers} from "../src/symbiotic/VotingPowers.sol";

import {BN254G2} from "./utils/BN254G2.sol";
import {MockERC20} from "./mock/MockERC20.sol";

/// @title RegisterOperators
/// @notice Register operators on testnet after relay infra is deployed.
///         Two-phase: fundOperators (deployer key) then registerOperator (operator key).
contract RegisterOperators is Script {
    using KeyTags for uint8;
    using KeyBlsBn254 for BN254.G1Point;
    using KeyBlsBn254 for KeyBlsBn254.KEY_BLS_BN254;
    using BN254 for BN254.G1Point;

    bytes32 internal constant KEY_OWNERSHIP_TYPEHASH = keccak256("KeyOwnership(address operator,bytes key)");

    uint8 internal constant REQUIRED_KEY_TAG_BLS = 15;
    uint8 internal constant REQUIRED_KEY_TAG_SECONDARY_BLS = 11;
    uint256 internal constant OPERATOR_STAKE_AMOUNT = 100_000 ether;
    uint256 internal constant OPERATOR_COUNT = 3;

    /// @notice Fund all operator addresses from deployer. Run with deployer private key.
    function fundOperators() external {
        string memory json = vm.readFile("deploy-data/relay_infra.json");
        address stakingTokenAddr = vm.parseJsonAddress(json, ".stakingToken");

        console.log("=== Funding Operators ===");
        console.log("StakingToken:", stakingTokenAddr);

        vm.startBroadcast();

        for (uint256 i = 0; i < OPERATOR_COUNT; i++) {
            uint256 baseKey = vm.envOr("OPERATOR_BASE_KEY", uint256(1e18));
            uint256 operatorPrivateKey = baseKey + i;
            address operatorAddr = vm.addr(operatorPrivateKey);

            console.log("Funding operator", i, ":", operatorAddr);

            (bool sent,) = payable(operatorAddr).call{value: 0.01 ether}("");
            require(sent, "Failed to fund operator ETH");

            MockERC20(stakingTokenAddr).transfer(operatorAddr, OPERATOR_STAKE_AMOUNT);
        }

        vm.stopBroadcast();
        console.log("=== Operators Funded ===");
    }

    /// @notice Register a single operator. Run with operator's private key.
    /// @param index Operator index (0, 1, 2)
    function registerOperator(uint256 index) external {
        string memory json = vm.readFile("deploy-data/relay_infra.json");

        OperatorRegistry operatorRegistry = OperatorRegistry(vm.parseJsonAddress(json, ".operatorRegistry"));
        address networkAddr = vm.parseJsonAddress(json, ".network");
        address stakingTokenAddr = vm.parseJsonAddress(json, ".stakingToken");
        KeyRegistry keyRegistry = KeyRegistry(vm.parseJsonAddress(json, ".keyRegistry"));
        VotingPowers votingPowers = VotingPowers(vm.parseJsonAddress(json, ".votingPowers"));

        // Load opt-in services from symbiotic core config (same as DeployRelayInfra)
        string memory coreConfig = vm.envOr("SYMBIOTIC_CORE_CONFIG", string(""));
        OptInService operatorVaultOptInService;
        OptInService operatorNetworkOptInService;
        if (bytes(coreConfig).length > 0) {
            string memory coreJson = vm.readFile(coreConfig);
            string memory chainKey = string(abi.encodePacked(".", vm.toString(block.chainid)));
            operatorVaultOptInService =
                OptInService(vm.parseJsonAddress(coreJson, string(abi.encodePacked(chainKey, ".operatorVaultOptInService"))));
            operatorNetworkOptInService =
                OptInService(vm.parseJsonAddress(coreJson, string(abi.encodePacked(chainKey, ".operatorNetworkOptInService"))));
        } else {
            revert("SYMBIOTIC_CORE_CONFIG not set");
        }

        uint256 baseKey = vm.envOr("OPERATOR_BASE_KEY", uint256(1e18));
        uint256 operatorPrivateKey = baseKey + index;
        address operatorAddr = vm.addr(operatorPrivateKey);

        console.log("=== Registering Operator", index, "===");
        console.log("Address:", operatorAddr);

        vm.startBroadcast(operatorPrivateKey);

        // Register operator
        operatorRegistry.registerOperator();
        operatorNetworkOptInService.optIn(networkAddr);
        votingPowers.registerOperator();

        // Get auto-deployed vault and opt-in
        IVault vault = IVault(votingPowers.getAutoDeployedVault(operatorAddr));
        operatorVaultOptInService.optIn(address(vault));

        // Deposit stake
        IERC20(stakingTokenAddr).approve(address(vault), OPERATOR_STAKE_AMOUNT);
        vault.deposit(operatorAddr, OPERATOR_STAKE_AMOUNT);

        // Register BLS key (tag 15)
        (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = _getBLSKeys(operatorPrivateKey);
        bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        bytes32 messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
        BN254.G1Point memory sigG1 = messageG1.scalar_mul(operatorPrivateKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_BLS), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        // Register secondary BLS key (tag 11)
        uint256 secondaryBLSKey = operatorPrivateKey + 10_000;
        (g1Key, g2Key) = _getBLSKeys(secondaryBLSKey);
        keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        messageG1 = BN254.hashToG1(messageHash);
        sigG1 = messageG1.scalar_mul(secondaryBLSKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_SECONDARY_BLS), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        vm.stopBroadcast();

        console.log("Operator registered with vault:", address(vault));
    }

    function _getBLSKeys(uint256 privateKey) internal view returns (BN254.G1Point memory, BN254.G2Point memory) {
        BN254.G1Point memory G1Key = BN254.generatorG1().scalar_mul(privateKey);
        BN254.G2Point memory G2 = BN254.generatorG2();
        (uint256 x1, uint256 x2, uint256 y1, uint256 y2) =
            BN254G2.ECTwistMul(privateKey, G2.X[1], G2.X[0], G2.Y[1], G2.Y[0]);
        return (G1Key, BN254.G2Point([x2, x1], [y2, y1]));
    }
}
