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

    // Configuration — defaults suitable for production; override via env for testnet.
    //   EPOCH_DURATION:    driver epoch length (default 28800 = 8 hours)
    //   SLASHING_WINDOW:   vault epoch / slashing window (default 86400 = 1 day)
    //   EPOCH_START_DELAY: seconds to delay epoch 0 start for operator registration (default 0 = immediate)
    uint48 internal constant DEFAULT_EPOCH_DURATION = 28800;
    uint48 internal constant DEFAULT_SLASHING_WINDOW = 86400;
    uint48 internal constant DEFAULT_EPOCH_START_DELAY = 0;
    uint208 internal constant MAX_VALIDATORS_COUNT = 1000;
    uint256 internal constant MAX_VOTING_POWER = 2 ** 247;
    uint256 internal constant MIN_INCLUSION_VOTING_POWER = 0;
    uint248 internal constant QUORUM_THRESHOLD = (uint248(1e18) * 2) / 3 + 1; // 66.67%
    uint8 internal constant REQUIRED_KEY_TAG_BLS = 15;
    uint8 internal constant REQUIRED_KEY_TAG_SECONDARY_BLS = 11;
    uint256 internal constant OPERATOR_STAKE_AMOUNT = 100_000 ether; // MockERC20, not real ETH
    uint256 internal constant OPERATOR_COUNT = 3; // 3 operators for quorum
    // Default threshold matches default fund amount so a single call converges.
    // xtask normally supplies this via env from the env JSON funding block.
    uint256 internal constant DEFAULT_EXTERNAL_MIN_NATIVE_BALANCE = 0.005 ether;

    // Resolved at runtime from env (or defaults above)
    uint48 internal epochDuration;
    uint48 internal slashingWindow;
    uint48 internal epochStartDelay;
    uint256 internal externalMinNativeBalance;

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
        deployer = msg.sender;

        // Resolve config from env (testnet overrides) or use production defaults
        epochDuration = uint48(vm.envOr("EPOCH_DURATION", uint256(DEFAULT_EPOCH_DURATION)));
        slashingWindow = uint48(vm.envOr("SLASHING_WINDOW", uint256(DEFAULT_SLASHING_WINDOW)));
        epochStartDelay = uint48(vm.envOr("EPOCH_START_DELAY", uint256(DEFAULT_EPOCH_START_DELAY)));
        externalMinNativeBalance =
            vm.envOr("EXTERNAL_MIN_NATIVE_BALANCE", DEFAULT_EXTERNAL_MIN_NATIVE_BALANCE);

        console.log("=== Deploying Symbiotic Relay Infrastructure ===");
        console.log("Chain ID:", block.chainid);
        console.log("Deployer:", deployer);
        console.log("Epoch duration:", uint256(epochDuration));
        console.log("Slashing window:", uint256(slashingWindow));
        console.log("Epoch start delay:", uint256(epochStartDelay));

        vm.startBroadcast();

        // Phase 1: Deploy or load Symbiotic Core
        _loadOrDeployCore();

        // Phase 2: Deploy Relay Infrastructure
        _deployStakingToken();
        _deployNetwork();
        _deployKeyRegistry();
        _deployVotingPowers();
        _deploySettlement();
        _deployDriver();

        // Phase 3: Register Operators
        vm.stopBroadcast();
        bool isLocal = block.chainid == 31337 || block.chainid == 31338;
        if (isLocal) {
            for (uint256 i = 0; i < OPERATOR_COUNT; i++) {
                _addLocalOperator(i, OPERATOR_STAKE_AMOUNT);
            }
            _fundExplicitSigners();
        } else {
            _registerExternalOperators();
            _fundExplicitSigners();
        }

        // Phase 4: Output deployment data
        _saveDeploymentData();

        console.log("");
        console.log("=== Deployment Complete ===");
    }

    function _loadOrDeployCore() internal {
        string memory coreConfigPath = vm.envOr("SYMBIOTIC_CORE_CONFIG", string(""));
        if (bytes(coreConfigPath).length > 0) {
            _loadCore(coreConfigPath);
        } else {
            _deployCore();
        }
    }

    function _loadCore(string memory configPath) internal {
        console.log("--- Loading Symbiotic Core from config ---");

        string memory json = vm.readFile(configPath);
        string memory chainKey = string(abi.encodePacked(".", vm.toString(block.chainid)));

        vaultFactory = VaultFactory(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".vaultFactory"))));
        delegatorFactory =
            DelegatorFactory(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".delegatorFactory"))));
        slasherFactory =
            SlasherFactory(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".slasherFactory"))));
        networkRegistry =
            NetworkRegistry(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".networkRegistry"))));
        operatorRegistry =
            OperatorRegistry(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".operatorRegistry"))));
        networkMiddlewareService = NetworkMiddlewareService(
            vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".networkMiddlewareService")))
        );
        operatorVaultOptInService =
            OptInService(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".operatorVaultOptInService"))));
        operatorNetworkOptInService =
            OptInService(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".operatorNetworkOptInService"))));
        vaultConfigurator =
            VaultConfigurator(vm.parseJsonAddress(json, string(abi.encodePacked(chainKey, ".vaultConfigurator"))));

        console.log("VaultFactory:", address(vaultFactory));
        console.log("NetworkRegistry:", address(networkRegistry));
        console.log("OperatorRegistry:", address(operatorRegistry));
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
                minVaultEpochDuration: slashingWindow,
                token: address(stakingToken)
            }),
            IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams({
                isAutoDeployEnabled: true,
                config: IOpNetVaultAutoDeploy.AutoDeployConfig({
                    epochDuration: slashingWindow,
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
                    epochDuration: epochDuration,
                    epochDurationTimestamp: (block.chainid == 31337 || block.chainid == 31338)
                        ? 0 // Local: start immediately (anvil can manipulate time)
                        : uint48(block.timestamp) + epochStartDelay // External: delay to allow operator registration
                }),
                numAggregators: 1,
                numCommitters: 1,
                committerSlotDuration: epochDuration,
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

    function _addLocalOperator(uint256 index, uint256 stakeAmount) internal {
        console.log("--- Adding Operator", index, "---");

        // Per-operator private key from env (OPERATOR_1_PRIVATE_KEY, etc.)
        uint256 operatorPrivateKey = _getOperatorKey(index);
        address operatorAddr = vm.addr(operatorPrivateKey);

        vm.startBroadcast();
        _ensureOperatorEth(operatorAddr);
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

    function _registerExternalOperators() internal {
        console.log("--- Registering External Operators ---");

        for (uint256 i = 0; i < OPERATOR_COUNT; i++) {
            _registerExternalOperator(i);
        }
    }

    function _registerExternalOperator(uint256 index) internal {
        uint256 operatorPrivateKey = _getOperatorKey(index);
        address operatorAddr = vm.addr(operatorPrivateKey);

        vm.startBroadcast();
        _ensureOperatorEth(operatorAddr);
        _ensureOperatorStake(operatorAddr);
        vm.stopBroadcast();

        vm.startBroadcast(operatorPrivateKey);

        if (!operatorRegistry.isEntity(operatorAddr)) {
            operatorRegistry.registerOperator();
        }
        if (!operatorNetworkOptInService.isOptedIn(operatorAddr, address(network))) {
            operatorNetworkOptInService.optIn(address(network));
        }

        // Idempotent: registerOperator reverts if already registered
        try votingPowers.registerOperator() {} catch {}

        IVault vault = IVault(votingPowers.getAutoDeployedVault(operatorAddr));
        if (!operatorVaultOptInService.isOptedIn(operatorAddr, address(vault))) {
            operatorVaultOptInService.optIn(address(vault));
        }

        uint256 tokenBalance = stakingToken.balanceOf(operatorAddr);
        if (tokenBalance > 0) {
            stakingToken.approve(address(vault), tokenBalance);
            vault.deposit(operatorAddr, tokenBalance);
        }

        _registerBlsKeys(operatorAddr, operatorPrivateKey);

        vm.stopBroadcast();

        console.log("Operator", index, "address:", operatorAddr);
        console.log("Operator", index, "vault:", address(vault));
    }

    function _ensureOperatorEth(address operatorAddr) internal {
        if (operatorAddr.balance >= externalMinNativeBalance) {
            return;
        }

        bool isLocal = block.chainid == 31337 || block.chainid == 31338;
        uint256 fundAmount = isLocal
            ? 1 ether
            : vm.envOr("OPERATOR_FUND_AMOUNT", uint256(0.005 ether));

        (bool success,) = operatorAddr.call{value: fundAmount}("");
        require(success, "failed to fund operator");
    }

    function _ensureOperatorStake(address operatorAddr) internal {
        if (stakingToken.balanceOf(operatorAddr) > 0) {
            return;
        }

        stakingToken.transfer(operatorAddr, OPERATOR_STAKE_AMOUNT);
    }

    function _fundExplicitSigners() internal {
        console.log("--- Funding Explicit Relayer Signers ---");
        bool isLocal = block.chainid == 31337 || block.chainid == 31338;
        uint256 fundAmount = isLocal
            ? 1 ether
            : vm.envOr("RELAYER_SIGNER_FUND_AMOUNT", uint256(0.005 ether));

        for (uint256 i = 1; i <= OPERATOR_COUNT; i++) {
            address signerAddr = vm.envOr(_signerEnvName(i), address(0));
            if (signerAddr == address(0)) {
                continue;
            }

            address operatorAddr = vm.addr(_getOperatorKey(i - 1));
            if (signerAddr == operatorAddr || signerAddr.balance >= externalMinNativeBalance) {
                continue;
            }

            vm.startBroadcast();
            (bool success,) = signerAddr.call{value: fundAmount}("");
            require(success, "failed to fund signer");
            vm.stopBroadcast();
            console.log("Funded signer", signerAddr);
        }
    }

    function _signerEnvName(uint256 index) internal pure returns (string memory) {
        if (index == 1) {
            return "SIGNER_1_ADDRESS";
        }
        if (index == 2) {
            return "SIGNER_2_ADDRESS";
        }
        if (index == 3) {
            return "SIGNER_3_ADDRESS";
        }
        revert("signer index out of range");
    }

    function _registerBlsKeys(address operatorAddr, uint256 operatorPrivateKey) internal {
        if (_isMissingKey(keyRegistry.getKey(operatorAddr, REQUIRED_KEY_TAG_BLS))) {
            (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = _getBLSKeys(operatorPrivateKey);
            bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
            bytes32 messageHash = keyRegistry.hashTypedDataV4(
                keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes)))
            );
            BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
            BN254.G1Point memory sigG1 = messageG1.scalar_mul(operatorPrivateKey);
            keyRegistry.setKey(KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_BLS), keyBytes, abi.encode(sigG1), abi.encode(g2Key));
        }

        if (_isMissingKey(keyRegistry.getKey(operatorAddr, REQUIRED_KEY_TAG_SECONDARY_BLS))) {
            uint256 secondaryBLSKey = operatorPrivateKey + 10_000;
            (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = _getBLSKeys(secondaryBLSKey);
            bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
            bytes32 messageHash = keyRegistry.hashTypedDataV4(
                keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operatorAddr, keccak256(keyBytes)))
            );
            BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
            BN254.G1Point memory sigG1 = messageG1.scalar_mul(secondaryBLSKey);
            keyRegistry.setKey(
                KEY_TYPE_BLS_BN254.getKeyTag(REQUIRED_KEY_TAG_SECONDARY_BLS),
                keyBytes,
                abi.encode(sigG1),
                abi.encode(g2Key)
            );
        }
    }

    function _isMissingKey(bytes memory key) internal pure returns (bool) {
        if (key.length == 0) {
            return true;
        }

        for (uint256 i = 0; i < key.length; i++) {
            if (key[i] != bytes1(0)) {
                return false;
            }
        }

        return true;
    }

    function _getOperatorKey(uint256 index) internal view returns (uint256) {
        string[3] memory envNames = ["OPERATOR_1_PRIVATE_KEY", "OPERATOR_2_PRIVATE_KEY", "OPERATOR_3_PRIVATE_KEY"];
        require(index < envNames.length, "operator index out of range");
        uint256 key = vm.envUint(envNames[index]);
        require(key != 0, string(abi.encodePacked(envNames[index], " is not set")));
        return key;
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
    }
}
