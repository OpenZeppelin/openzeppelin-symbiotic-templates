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
            uint256 operatorPrivateKey = _getOperatorKey(i);
            address operatorAddr = vm.addr(operatorPrivateKey);

            console.log("Funding operator", i, ":", operatorAddr);

            (bool sent,) = payable(operatorAddr).call{value: 0.01 ether}("");
            require(sent, "Failed to fund operator ETH");

            MockERC20(stakingTokenAddr).transfer(operatorAddr, OPERATOR_STAKE_AMOUNT);
        }

        vm.stopBroadcast();
        console.log("=== Operators Funded ===");
    }

    struct Contracts {
        OperatorRegistry operatorRegistry;
        OptInService networkOptIn;
        OptInService vaultOptIn;
        VotingPowers votingPowers;
        KeyRegistry keyRegistry;
        address networkAddr;
        address stakingTokenAddr;
    }

    function _loadContracts() internal returns (Contracts memory c) {
        string memory json = vm.readFile("deploy-data/relay_infra.json");
        c.operatorRegistry = OperatorRegistry(vm.parseJsonAddress(json, ".operatorRegistry"));
        c.networkAddr = vm.parseJsonAddress(json, ".network");
        c.stakingTokenAddr = vm.parseJsonAddress(json, ".stakingToken");
        c.keyRegistry = KeyRegistry(vm.parseJsonAddress(json, ".keyRegistry"));
        c.votingPowers = VotingPowers(vm.parseJsonAddress(json, ".votingPowers"));

        string memory coreConfig = vm.envOr("SYMBIOTIC_CORE_CONFIG", string(""));
        require(bytes(coreConfig).length > 0, "SYMBIOTIC_CORE_CONFIG not set");
        string memory coreJson = vm.readFile(coreConfig);
        string memory chainKey = string(abi.encodePacked(".", vm.toString(block.chainid)));
        c.vaultOptIn = OptInService(vm.parseJsonAddress(coreJson, string(abi.encodePacked(chainKey, ".operatorVaultOptInService"))));
        c.networkOptIn = OptInService(vm.parseJsonAddress(coreJson, string(abi.encodePacked(chainKey, ".operatorNetworkOptInService"))));
    }

    /// @notice Register all operators in one script execution (faster than 3 separate calls).
    ///         Each operator broadcasts with its own key. Minimizes the epoch gap.
    function registerAllOperators() external {
        Contracts memory c = _loadContracts();

        for (uint256 i = 0; i < OPERATOR_COUNT; i++) {
            uint256 opKey = _getOperatorKey(i);
            address opAddr = vm.addr(opKey);
            console.log("Registering operator", i, ":", opAddr);

            vm.startBroadcast(opKey);
            _registerInRegistries(c.operatorRegistry, c.networkOptIn, c.vaultOptIn, c.votingPowers, c.networkAddr, opAddr, c.stakingTokenAddr);
            _registerBLSKeys(c.keyRegistry, opAddr, opKey);
            vm.stopBroadcast();
        }
        console.log("All operators registered");
    }

    /// @notice Register a single operator. Run with operator's private key.
    /// @param index Operator index (0, 1, 2)
    function registerOperator(uint256 index) external {
        Contracts memory c = _loadContracts();

        uint256 operatorPrivateKey = _getOperatorKey(index);
        address operatorAddr = vm.addr(operatorPrivateKey);

        console.log("=== Registering Operator", index, "===");
        console.log("Address:", operatorAddr);

        vm.startBroadcast(operatorPrivateKey);

        // Register operator (idempotent — skip steps already done on shared registries)
        _registerInRegistries(c.operatorRegistry, c.networkOptIn, c.vaultOptIn, c.votingPowers, c.networkAddr, operatorAddr, c.stakingTokenAddr);

        // Register BLS keys on our KeyRegistry (fresh per relay infra deploy)
        _registerBLSKeys(c.keyRegistry, operatorAddr, operatorPrivateKey);

        vm.stopBroadcast();

        console.log("Operator registered");
    }

    function _registerInRegistries(
        OperatorRegistry operatorRegistry,
        OptInService networkOptIn,
        OptInService vaultOptIn,
        VotingPowers votingPowers,
        address networkAddr,
        address operatorAddr,
        address stakingTokenAddr
    ) internal {
        // Shared OperatorRegistry — operator may already be registered from a previous deploy
        if (!operatorRegistry.isEntity(operatorAddr)) {
            operatorRegistry.registerOperator();
        }
        if (!networkOptIn.isOptedIn(operatorAddr, networkAddr)) {
            networkOptIn.optIn(networkAddr);
        }

        // VotingPowers is per-deploy, but use try/catch for safety
        try votingPowers.registerOperator() {} catch {}

        // Vault opt-in and stake deposit
        IVault vault = IVault(votingPowers.getAutoDeployedVault(operatorAddr));
        if (!vaultOptIn.isOptedIn(operatorAddr, address(vault))) {
            vaultOptIn.optIn(address(vault));
        }
        uint256 tokenBalance = IERC20(stakingTokenAddr).balanceOf(operatorAddr);
        if (tokenBalance > 0) {
            IERC20(stakingTokenAddr).approve(address(vault), tokenBalance);
            vault.deposit(operatorAddr, tokenBalance);
        }
    }

    function _getOperatorKey(uint256 index) internal view returns (uint256) {
        string[3] memory envNames = ["OPERATOR_1_PRIVATE_KEY", "OPERATOR_2_PRIVATE_KEY", "OPERATOR_3_PRIVATE_KEY"];
        require(index < envNames.length, "operator index out of range");
        uint256 key = vm.envUint(envNames[index]);
        require(key != 0, string(abi.encodePacked(envNames[index], " is not set")));
        return key;
    }

    function _registerBLSKeys(KeyRegistry keyRegistry, address operatorAddr, uint256 operatorPrivateKey) internal {
        // Primary BLS key (tag 15)
        (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = _getBLSKeys(operatorPrivateKey);
        bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        bytes32 messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
        BN254.G1Point memory sigG1 = messageG1.scalar_mul(operatorPrivateKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_BLS), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        // Secondary BLS key (tag 11)
        uint256 secondaryBLSKey = operatorPrivateKey + 10_000;
        (g1Key, g2Key) = _getBLSKeys(secondaryBLSKey);
        keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        messageG1 = BN254.hashToG1(messageHash);
        sigG1 = messageG1.scalar_mul(secondaryBLSKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_SECONDARY_BLS), keyBytes, abi.encode(sigG1), abi.encode(g2Key));
    }

    function _getBLSKeys(uint256 privateKey) internal view returns (BN254.G1Point memory, BN254.G2Point memory) {
        BN254.G1Point memory G1Key = BN254.generatorG1().scalar_mul(privateKey);
        BN254.G2Point memory G2 = BN254.generatorG2();
        (uint256 x1, uint256 x2, uint256 y1, uint256 y2) =
            BN254G2.ECTwistMul(privateKey, G2.X[1], G2.X[0], G2.Y[1], G2.Y[0]);
        return (G1Key, BN254.G2Point([x2, x1], [y2, y1]));
    }
}
