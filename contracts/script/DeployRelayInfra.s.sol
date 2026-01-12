// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {Vm} from "forge-std/Vm.sol";

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

// Symbiotic Core Imports
import {VaultFactory} from "@symbioticfi/core/src/contracts/VaultFactory.sol";
import {DelegatorFactory} from "@symbioticfi/core/src/contracts/DelegatorFactory.sol";
import {SlasherFactory} from "@symbioticfi/core/src/contracts/SlasherFactory.sol";
import {NetworkRegistry} from "@symbioticfi/core/src/contracts/NetworkRegistry.sol";
import {OperatorRegistry} from "@symbioticfi/core/src/contracts/OperatorRegistry.sol";
import {MetadataService} from "@symbioticfi/core/src/contracts/service/MetadataService.sol";
import {NetworkMiddlewareService} from "@symbioticfi/core/src/contracts/service/NetworkMiddlewareService.sol";
import {OptInService} from "@symbioticfi/core/src/contracts/service/OptInService.sol";
import {VaultConfigurator} from "@symbioticfi/core/src/contracts/VaultConfigurator.sol";
import {Vault} from "@symbioticfi/core/src/contracts/vault/Vault.sol";
import {NetworkRestakeDelegator} from "@symbioticfi/core/src/contracts/delegator/NetworkRestakeDelegator.sol";
import {FullRestakeDelegator} from "@symbioticfi/core/src/contracts/delegator/FullRestakeDelegator.sol";
import {OperatorSpecificDelegator} from "@symbioticfi/core/src/contracts/delegator/OperatorSpecificDelegator.sol";
import {OperatorNetworkSpecificDelegator} from "@symbioticfi/core/src/contracts/delegator/OperatorNetworkSpecificDelegator.sol";
import {Slasher} from "@symbioticfi/core/src/contracts/slasher/Slasher.sol";

import {IVault} from "@symbioticfi/core/src/interfaces/vault/IVault.sol";
import {INetworkMiddlewareService} from "@symbioticfi/core/src/interfaces/service/INetworkMiddlewareService.sol";
import {IVaultConfigurator} from "@symbioticfi/core/src/interfaces/IVaultConfigurator.sol";
import {IDelegatorFactory} from "@symbioticfi/core/src/interfaces/IDelegatorFactory.sol";
import {ISlasherFactory} from "@symbioticfi/core/src/interfaces/ISlasherFactory.sol";

// Relay Contracts Imports
import {INetwork} from "@symbioticfi/network/src/interfaces/INetwork.sol";
import {Network} from "@symbioticfi/network/src/Network.sol";
import {INetworkManager} from "@symbioticfi/relay-contracts/interfaces/modules/base/INetworkManager.sol";
import {IKeyRegistry} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";
import {IEpochManager} from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IEpochManager.sol";
import {IValSetDriver} from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IValSetDriver.sol";
import {IVotingPowerProvider} from "@symbioticfi/relay-contracts/interfaces/modules/voting-power/IVotingPowerProvider.sol";
import {IOpNetVaultAutoDeploy} from
    "@symbioticfi/relay-contracts/interfaces/modules/voting-power/extensions/IOpNetVaultAutoDeploy.sol";
import {SigVerifierBlsBn254Simple} from
    "@symbioticfi/relay-contracts/modules/settlement/sig-verifiers/SigVerifierBlsBn254Simple.sol";
import {ISettlement} from "@symbioticfi/relay-contracts/interfaces/modules/settlement/ISettlement.sol";
import {IOzOwnable} from "@symbioticfi/relay-contracts/interfaces/modules/common/permissions/IOzOwnable.sol";
import {IOzEIP712} from "@symbioticfi/relay-contracts/interfaces/modules/base/IOzEIP712.sol";
import {KeyTags} from "@symbioticfi/relay-contracts/libraries/utils/KeyTags.sol";
import {KeyBlsBn254, BN254} from "@symbioticfi/relay-contracts/libraries/keys/KeyBlsBn254.sol";
import {KEY_TYPE_BLS_BN254} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";

import {KeyRegistry} from "../src/symbiotic/KeyRegistry.sol";
import {Driver} from "../src/symbiotic/Driver.sol";
import {VotingPowers} from "../src/symbiotic/VotingPowers.sol";
import {Settlement} from "../src/symbiotic/Settlement.sol";

import {BN254G2} from "./utils/BN254G2.sol";
import {MockERC20} from "./mock/MockERC20.sol";

/// @title DeployRelayInfra
/// @notice Deploy full Symbiotic relay infrastructure for E2E testing
/// @dev Adapted from symbiotic-super-sum LocalDeploy.s.sol
contract DeployRelayInfra is Script {
    using KeyTags for uint8;
    using KeyBlsBn254 for BN254.G1Point;
    using KeyBlsBn254 for KeyBlsBn254.KEY_BLS_BN254;
    using BN254 for BN254.G1Point;

    bytes32 internal constant KEY_OWNERSHIP_TYPEHASH = keccak256("KeyOwnership(address operator,bytes key)");

    // Configuration
    uint48 internal constant EPOCH_DURATION = 60; // 1 minute epochs for testing
    uint48 internal constant SLASHING_WINDOW = 1 days;
    uint208 internal constant MAX_VALIDATORS_COUNT = 1000;
    uint256 internal constant MAX_VOTING_POWER = 2 ** 247;
    uint256 internal constant MIN_INCLUSION_VOTING_POWER = 0;
    uint248 internal constant QUORUM_THRESHOLD = (uint248(1e18) * 2) / 3 + 1; // 66.67%
    uint8 internal constant REQUIRED_KEY_TAG_BLS = 15;
    uint8 internal constant REQUIRED_KEY_TAG_SECONDARY_BLS = 11;
    uint256 internal constant OPERATOR_STAKE_AMOUNT = 100_000 ether;
    uint256 internal constant OPERATOR_COUNT = 3; // 3 operators for quorum

    address internal deployer;

    // Symbiotic Core
    VaultFactory internal vaultFactory;
    DelegatorFactory internal delegatorFactory;
    SlasherFactory internal slasherFactory;
    NetworkRegistry internal networkRegistry;
    OperatorRegistry internal operatorRegistry;
    NetworkMiddlewareService internal networkMiddlewareService;
    OptInService internal operatorVaultOptInService;
    OptInService internal operatorNetworkOptInService;
    VaultConfigurator internal vaultConfigurator;

    // Relay Infrastructure
    Network internal network;
    KeyRegistry internal keyRegistry;
    VotingPowers internal votingPowers;
    Settlement internal settlement;
    Driver internal driver;
    MockERC20 internal stakingToken;

    function run() external {
        deployer = vm.envOr("DEPLOYER_ADDRESS", address(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266));

        console.log("=== Deploying Symbiotic Relay Infrastructure ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);

        vm.startBroadcast(deployer);

        // Phase 1: Deploy Symbiotic Core
        _deployCore();

        // Phase 2: Deploy Relay Infrastructure
        _deployStakingToken();
        _deployNetwork();
        _deployKeyRegistry();
        _deployVotingPowers();
        _deploySettlement();
        _deployDriver();

        // Phase 3: Register Operators
        vm.stopBroadcast();
        for (uint256 i = 0; i < OPERATOR_COUNT; i++) {
            _addOperator(i, OPERATOR_STAKE_AMOUNT);
        }

        // Phase 4: Output deployment data
        _saveDeploymentData();

        console.log("");
        console.log("=== Deployment Complete ===");
    }

    function _deployCore() internal {
        console.log("--- Deploying Symbiotic Core ---");

        // Factories
        vaultFactory = new VaultFactory(deployer);
        delegatorFactory = new DelegatorFactory(deployer);
        slasherFactory = new SlasherFactory(deployer);

        // Registries
        networkRegistry = new NetworkRegistry();
        operatorRegistry = new OperatorRegistry();

        // Services
        networkMiddlewareService = new NetworkMiddlewareService(address(networkRegistry));
        operatorVaultOptInService =
            new OptInService(address(operatorRegistry), address(vaultFactory), "OperatorVaultOptInService");
        operatorNetworkOptInService =
            new OptInService(address(operatorRegistry), address(networkRegistry), "OperatorNetworkOptInService");

        // Vault implementation
        address vaultImpl = address(new Vault(address(delegatorFactory), address(slasherFactory), address(vaultFactory)));
        vaultFactory.whitelist(vaultImpl);

        // Delegator implementations (need all 4 types for VotingPowers auto-deploy)
        // Type 0: NetworkRestakeDelegator
        address delegatorImpl0 = address(
            new NetworkRestakeDelegator(
                address(networkRegistry),
                address(vaultFactory),
                address(operatorVaultOptInService),
                address(operatorNetworkOptInService),
                address(delegatorFactory),
                0 // delegatorType
            )
        );
        delegatorFactory.whitelist(delegatorImpl0);

        // Type 1: FullRestakeDelegator
        address delegatorImpl1 = address(
            new FullRestakeDelegator(
                address(networkRegistry),
                address(vaultFactory),
                address(operatorVaultOptInService),
                address(operatorNetworkOptInService),
                address(delegatorFactory),
                1 // delegatorType
            )
        );
        delegatorFactory.whitelist(delegatorImpl1);

        // Type 2: OperatorSpecificDelegator (has extra operatorRegistry param)
        address delegatorImpl2 = address(
            new OperatorSpecificDelegator(
                address(operatorRegistry),
                address(networkRegistry),
                address(vaultFactory),
                address(operatorVaultOptInService),
                address(operatorNetworkOptInService),
                address(delegatorFactory),
                2 // delegatorType
            )
        );
        delegatorFactory.whitelist(delegatorImpl2);

        // Type 3: OperatorNetworkSpecificDelegator (has extra operatorRegistry param)
        address delegatorImpl3 = address(
            new OperatorNetworkSpecificDelegator(
                address(operatorRegistry),
                address(networkRegistry),
                address(vaultFactory),
                address(operatorVaultOptInService),
                address(operatorNetworkOptInService),
                address(delegatorFactory),
                3 // delegatorType
            )
        );
        delegatorFactory.whitelist(delegatorImpl3);

        // Slasher implementation
        address slasherImpl = address(
            new Slasher(
                address(vaultFactory),
                address(networkMiddlewareService),
                address(slasherFactory),
                0 // slasherType
            )
        );
        slasherFactory.whitelist(slasherImpl);

        // Vault Configurator
        vaultConfigurator =
            new VaultConfigurator(address(vaultFactory), address(delegatorFactory), address(slasherFactory));

        console.log("VaultFactory:", address(vaultFactory));
        console.log("NetworkRegistry:", address(networkRegistry));
        console.log("OperatorRegistry:", address(operatorRegistry));
    }

    function _deployStakingToken() internal {
        console.log("--- Deploying Staking Token ---");
        stakingToken = new MockERC20("StakingToken", "STK");
        console.log("StakingToken:", address(stakingToken));
    }

    function _deployNetwork() internal {
        console.log("--- Deploying Network ---");

        network = new Network(address(networkRegistry), address(networkMiddlewareService));

        address[] memory proposersAndExecutors = new address[](1);
        proposersAndExecutors[0] = deployer;

        network.initialize(
            INetwork.NetworkInitParams({
                globalMinDelay: 0,
                delayParams: new INetwork.DelayParams[](0),
                proposers: proposersAndExecutors,
                executors: proposersAndExecutors,
                name: "Symbiotic DVN Network",
                metadataURI: "https://symbiotic-dvn.test",
                defaultAdminRoleHolder: deployer,
                nameUpdateRoleHolder: deployer,
                metadataURIUpdateRoleHolder: deployer
            })
        );
        console.log("Network:", address(network));
    }

    function _deployKeyRegistry() internal {
        console.log("--- Deploying KeyRegistry ---");

        keyRegistry = new KeyRegistry();
        keyRegistry.initialize(
            IKeyRegistry.KeyRegistryInitParams({
                ozEip712InitParams: IOzEIP712.OzEIP712InitParams({name: "KeyRegistry", version: "1"})
            })
        );
        console.log("KeyRegistry:", address(keyRegistry));
    }

    function _deployVotingPowers() internal {
        console.log("--- Deploying VotingPowers ---");

        votingPowers = new VotingPowers(
            address(operatorRegistry), address(vaultFactory), address(vaultConfigurator)
        );

        votingPowers.initialize(
            IVotingPowerProvider.VotingPowerProviderInitParams({
                networkManagerInitParams: INetworkManager.NetworkManagerInitParams({
                    network: address(network),
                    subnetworkId: 0
                }),
                ozEip712InitParams: IOzEIP712.OzEIP712InitParams({name: "VotingPowers", version: "1"}),
                requireSlasher: false,
                minVaultEpochDuration: SLASHING_WINDOW,
                token: address(stakingToken)
            }),
            IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams({
                isAutoDeployEnabled: true,
                config: IOpNetVaultAutoDeploy.AutoDeployConfig({
                    epochDuration: SLASHING_WINDOW,
                    collateral: address(stakingToken),
                    burner: address(0),
                    withSlasher: true,
                    isBurnerHook: false
                }),
                isSetMaxNetworkLimitHookEnabled: true
            }),
            IOzOwnable.OzOwnableInitParams({owner: deployer})
        );

        // Set VotingPowers as middleware for the network
        network.schedule(
            address(networkMiddlewareService),
            0,
            abi.encodeWithSelector(INetworkMiddlewareService.setMiddleware.selector, address(votingPowers)),
            bytes32(0),
            bytes32(0),
            0
        );
        network.execute(
            address(networkMiddlewareService),
            0,
            abi.encodeWithSelector(INetworkMiddlewareService.setMiddleware.selector, address(votingPowers)),
            bytes32(0),
            bytes32(0)
        );

        console.log("VotingPowers:", address(votingPowers));
    }

    function _deploySettlement() internal {
        console.log("--- Deploying Settlement ---");

        settlement = new Settlement();
        address verifier = address(new SigVerifierBlsBn254Simple());

        settlement.initialize(
            ISettlement.SettlementInitParams({
                networkManagerInitParams: INetworkManager.NetworkManagerInitParams({
                    network: address(network),
                    subnetworkId: 0
                }),
                ozEip712InitParams: IOzEIP712.OzEIP712InitParams({name: "Settlement", version: "1"}),
                sigVerifier: verifier
            }),
            deployer
        );

        console.log("Settlement:", address(settlement));
        console.log("SigVerifier:", verifier);
    }

    function _deployDriver() internal {
        console.log("--- Deploying Driver ---");

        driver = new Driver();

        // Set up voting power providers
        IValSetDriver.CrossChainAddress[] memory votingPowerProviders = new IValSetDriver.CrossChainAddress[](1);
        votingPowerProviders[0] =
            IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(votingPowers)});

        // Set up settlements
        IValSetDriver.CrossChainAddress[] memory settlements = new IValSetDriver.CrossChainAddress[](1);
        settlements[0] = IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(settlement)});

        // Set up quorum thresholds (BLS keys only)
        IValSetDriver.QuorumThreshold[] memory quorumThresholds = new IValSetDriver.QuorumThreshold[](2);
        quorumThresholds[0] =
            IValSetDriver.QuorumThreshold({keyTag: REQUIRED_KEY_TAG_BLS, quorumThreshold: QUORUM_THRESHOLD});
        quorumThresholds[1] =
            IValSetDriver.QuorumThreshold({keyTag: REQUIRED_KEY_TAG_SECONDARY_BLS, quorumThreshold: QUORUM_THRESHOLD});

        // Required key tags (BLS keys only)
        uint8[] memory requiredKeyTags = new uint8[](2);
        requiredKeyTags[0] = REQUIRED_KEY_TAG_BLS;
        requiredKeyTags[1] = REQUIRED_KEY_TAG_SECONDARY_BLS;

        driver.initialize(
            IValSetDriver.ValSetDriverInitParams({
                networkManagerInitParams: INetworkManager.NetworkManagerInitParams({
                    network: address(network),
                    subnetworkId: 0
                }),
                epochManagerInitParams: IEpochManager.EpochManagerInitParams({
                    epochDuration: EPOCH_DURATION,
                    epochDurationTimestamp: 0 // Use 0 so contract uses block.timestamp at execution time
                }),
                numAggregators: 1,
                numCommitters: 1,
                committerSlotDuration: EPOCH_DURATION,
                votingPowerProviders: votingPowerProviders,
                keysProvider: IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(keyRegistry)}),
                settlements: settlements,
                maxVotingPower: MAX_VOTING_POWER,
                minInclusionVotingPower: MIN_INCLUSION_VOTING_POWER,
                maxValidatorsCount: MAX_VALIDATORS_COUNT,
                requiredKeyTags: requiredKeyTags,
                quorumThresholds: quorumThresholds,
                requiredHeaderKeyTag: REQUIRED_KEY_TAG_BLS,
                verificationType: 1 // Simple BLS verification
            }),
            deployer
        );

        console.log("Driver:", address(driver));
    }

    function _addOperator(uint256 index, uint256 stakeAmount) internal {
        console.log("--- Adding Operator", index, "---");

        // Deterministic operator key (same as symbiotic-super-sum)
        uint256 operatorPrivateKey = 1e18 + index;
        address operatorAddr = vm.addr(operatorPrivateKey);

        vm.startBroadcast(deployer);
        // Fund operator
        payable(operatorAddr).transfer(1 ether);
        stakingToken.transfer(operatorAddr, stakeAmount);
        vm.stopBroadcast();

        vm.startBroadcast(operatorPrivateKey);

        // Register operator
        operatorRegistry.registerOperator();
        operatorNetworkOptInService.optIn(address(network));
        votingPowers.registerOperator();

        // Get auto-deployed vault and opt-in
        IVault vault = IVault(votingPowers.getAutoDeployedVault(operatorAddr));
        operatorVaultOptInService.optIn(address(vault));

        // Deposit stake
        stakingToken.approve(address(vault), stakeAmount);
        vault.deposit(operatorAddr, stakeAmount);

        // Register BLS key (tag 15)
        (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = _getBLSKeys(operatorPrivateKey);
        bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        bytes32 messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
        BN254.G1Point memory sigG1 = messageG1.scalar_mul(operatorPrivateKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(15), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        // Register secondary BLS key (tag 11)
        uint256 secondaryBLSKey = operatorPrivateKey + 10_000;
        (g1Key, g2Key) = _getBLSKeys(secondaryBLSKey);
        keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        messageHash =
            keyRegistry.hashTypedDataV4(keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes))));
        messageG1 = BN254.hashToG1(messageHash);
        sigG1 = messageG1.scalar_mul(secondaryBLSKey);
        keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(11), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        vm.stopBroadcast();

        console.log("Operator", index, "address:", operatorAddr);
        console.log("Operator", index, "vault:", address(vault));
    }

    function _getBLSKeys(uint256 privateKey) internal view returns (BN254.G1Point memory, BN254.G2Point memory) {
        BN254.G1Point memory G1Key = BN254.generatorG1().scalar_mul(privateKey);
        BN254.G2Point memory G2 = BN254.generatorG2();
        (uint256 x1, uint256 x2, uint256 y1, uint256 y2) =
            BN254G2.ECTwistMul(privateKey, G2.X[1], G2.X[0], G2.Y[1], G2.Y[0]);
        return (G1Key, BN254.G2Point([x2, x1], [y2, y1]));
    }

    function _saveDeploymentData() internal {
        console.log("--- Saving Deployment Data ---");

        string memory obj = "relayInfra";

        vm.serializeUint(obj, "chainId", block.chainid);
        vm.serializeAddress(obj, "network", address(network));
        vm.serializeAddress(obj, "keyRegistry", address(keyRegistry));
        vm.serializeAddress(obj, "votingPowers", address(votingPowers));
        vm.serializeAddress(obj, "settlement", address(settlement));
        vm.serializeAddress(obj, "driver", address(driver));
        vm.serializeAddress(obj, "stakingToken", address(stakingToken));

        // Symbiotic core addresses
        vm.serializeAddress(obj, "vaultFactory", address(vaultFactory));
        vm.serializeAddress(obj, "operatorRegistry", address(operatorRegistry));
        string memory finalJson = vm.serializeAddress(obj, "networkRegistry", address(networkRegistry));

        vm.writeJson(finalJson, "deploy-data/relay_infra.json");
        console.log("Saved to deploy-data/relay_infra.json");

        // Create deployment complete marker
        vm.writeFile("deploy-data/relay-infra-complete.marker", vm.toString(block.timestamp));
    }
}
