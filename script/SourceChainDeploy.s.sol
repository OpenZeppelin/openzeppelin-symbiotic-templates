// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {console} from "forge-std/console.sol";
import {Vm} from "forge-std/Vm.sol";

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {EnumerableMap} from "@openzeppelin/contracts/utils/structs/EnumerableMap.sol";

import {SymbioticCoreInit} from "@symbioticfi/core-contracts/script/integration/SymbioticCoreInit.sol";
import {IVault} from "@symbioticfi/core-contracts/src/interfaces/vault/IVault.sol";
import {INetworkMiddlewareService} from
    "@symbioticfi/core-contracts/src/interfaces/service/INetworkMiddlewareService.sol";

import {INetwork} from "@symbioticfi/network/src/interfaces/INetwork.sol";
import {INetworkManager} from "@symbioticfi/relay-contracts/interfaces/modules/base/INetworkManager.sol";
import {IKeyRegistry} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";
import {IValSetDriver} from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IValSetDriver.sol";
import {IVotingPowerProvider} from
    "@symbioticfi/relay-contracts/interfaces/modules/voting-power/IVotingPowerProvider.sol";
import {IOpNetVaultAutoDeploy} from
    "@symbioticfi/relay-contracts/interfaces/modules/voting-power/extensions/IOpNetVaultAutoDeploy.sol";
import {IOzOwnable} from "@symbioticfi/relay-contracts/interfaces/modules/common/permissions/IOzOwnable.sol";
import {IOzEIP712} from "@symbioticfi/relay-contracts/interfaces/modules/base/IOzEIP712.sol";
import {KeyTags} from "@symbioticfi/relay-contracts/libraries/utils/KeyTags.sol";
import {KeyBlsBn254, BN254} from "@symbioticfi/relay-contracts/libraries/keys/KeyBlsBn254.sol";
import {KEY_TYPE_BLS_BN254} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";

import {BN254G2} from "./utils/BN254G2.sol";
import {MockERC20} from "./mock/MockERC20.sol";

import {Network} from "@symbioticfi/network/src/Network.sol";
import {KeyRegistry} from "../src/symbiotic/KeyRegistry.sol";
import {VotingPowers} from "../src/symbiotic/VotingPowers.sol";
import {SymbioticLayerZeroDVN} from "../src/SymbioticLayerZeroDVN.sol";

/// @title SourceChainDeploy
/// @notice Phase 1: Deploy Symbiotic Core + Network + KeyRegistry + VotingPowers + DVN on source chain
/// @dev This deploys everything EXCEPT Settlement and Driver (Settlement on dest, Driver after Settlements)
contract SourceChainDeploy is SymbioticCoreInit {
    using KeyTags for uint8;
    using KeyBlsBn254 for BN254.G1Point;
    using BN254 for BN254.G1Point;
    using KeyBlsBn254 for KeyBlsBn254.KEY_BLS_BN254;
    using EnumerableMap for EnumerableMap.UintToAddressMap;

    struct CrossChainAddress {
        address addr;
        uint64 chainId;
    }

    // NOTE: Fields MUST be in alphabetical order for Foundry's parseJson/abi.decode
    struct SourceChainContracts {
        CrossChainAddress dvn;
        CrossChainAddress keyRegistry;
        address network;
        CrossChainAddress stakingToken;
        CrossChainAddress votingPowerProvider;
    }

    bytes32 internal constant KEY_OWNERSHIP_TYPEHASH = keccak256("KeyOwnership(address operator,bytes key)");

    uint48 internal immutable EPOCH_DURATION = uint48(vm.envOr("EPOCH_TIME", uint256(60)));
    uint48 internal constant SLASHING_WINDOW = 1 days;
    uint256 internal constant OPERATOR_STAKE_AMOUNT = 100_000;
    uint256 internal immutable OPERATOR_COUNT = vm.envOr("OPERATOR_COUNT", uint256(4));
    uint256 internal constant DVN_BASE_FEE = 0.001 ether;

    address internal deployer;

    Network internal network;
    IValSetDriver.CrossChainAddress internal keyRegistry;
    EnumerableMap.UintToAddressMap internal stakingTokens;
    EnumerableMap.UintToAddressMap internal votingPowerProviders;
    EnumerableMap.UintToAddressMap internal dvns;

    uint256 internal operatorsCount;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266);
    }

    function run(uint256) public override {
        deployer = getDeployerAddress();

        SYMBIOTIC_CORE_PROJECT_ROOT = "node_modules/@symbioticfi/core/";

        console.log("=== Phase 1: Source Chain Deployment ===");
        console.log("Chain ID:", block.chainid);

        setupCore();
        setupStakingToken();
        setupNetwork();
        setupKeyRegistry();
        setupVotingPowers();
        setupDVN();

        logAndDumpSourceChainContracts();

        // Add operators
        for (uint256 i; i < OPERATOR_COUNT; ++i) {
            addOperator(OPERATOR_STAKE_AMOUNT);
        }
        printOperatorsInfo();

        console.log("");
        console.log("=== Source Chain Deployment Complete ===");
        console.log("Next: Run DestinationChainDeploy.s.sol on destination chain");
    }

    function setupCore() public {
        vm.startBroadcast(deployer);
        _initCore_SymbioticCore(false);
        vm.stopBroadcast();

        console.log("Symbiotic Core contracts:");
        console.log("   VaultFactory:", address(symbioticCore.vaultFactory));
        console.log("   OperatorRegistry:", address(symbioticCore.operatorRegistry));
        console.log("   NetworkMiddlewareService:", address(symbioticCore.networkMiddlewareService));
    }

    function setupStakingToken() public returns (IValSetDriver.CrossChainAddress memory) {
        vm.startBroadcast(deployer);
        MockERC20 stakingToken = new MockERC20("StakingToken", "STK");
        stakingTokens.set(block.chainid, address(stakingToken));
        vm.stopBroadcast();

        console.log("StakingToken:", address(stakingToken));
        return IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(stakingToken)});
    }

    function setupNetwork() public returns (address) {
        vm.startBroadcast(deployer);
        network = new Network(address(symbioticCore.networkRegistry), address(symbioticCore.networkMiddlewareService));
        address[] memory proposersAndExecutors = new address[](1);
        proposersAndExecutors[0] = deployer;

        network.initialize(
            INetwork.NetworkInitParams({
                globalMinDelay: 0,
                delayParams: new INetwork.DelayParams[](0),
                proposers: proposersAndExecutors,
                executors: proposersAndExecutors,
                name: "LayerZero DVN Network",
                metadataURI: "https://example.network",
                defaultAdminRoleHolder: deployer,
                nameUpdateRoleHolder: deployer,
                metadataURIUpdateRoleHolder: deployer
            })
        );
        vm.stopBroadcast();

        console.log("Network:", address(network));
        return address(network);
    }

    function setupKeyRegistry() public returns (IValSetDriver.CrossChainAddress memory) {
        vm.startBroadcast(deployer);
        KeyRegistry keyRegistry_ = new KeyRegistry{salt: "KeyRegistry"}();
        keyRegistry_.initialize(
            IKeyRegistry.KeyRegistryInitParams({
                ozEip712InitParams: IOzEIP712.OzEIP712InitParams({name: "KeyRegistry", version: "1"})
            })
        );
        keyRegistry = IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(keyRegistry_)});
        vm.stopBroadcast();

        console.log("KeyRegistry:", address(keyRegistry_));
        return IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(keyRegistry_)});
    }

    function setupVotingPowers() public returns (IValSetDriver.CrossChainAddress memory) {
        IERC20 stakingToken = IERC20(stakingTokens.get(block.chainid));

        vm.startBroadcast(deployer);
        VotingPowers votingPowers_ = new VotingPowers{salt: "VotingPowers"}(
            address(symbioticCore.operatorRegistry),
            address(symbioticCore.vaultFactory),
            address(symbioticCore.vaultConfigurator)
        );
        votingPowers_.initialize(
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

        network.schedule(
            address(symbioticCore.networkMiddlewareService),
            0,
            abi.encodeWithSelector(INetworkMiddlewareService.setMiddleware.selector, address(votingPowers_)),
            bytes32(0),
            bytes32(0),
            0
        );

        network.execute(
            address(symbioticCore.networkMiddlewareService),
            0,
            abi.encodeWithSelector(INetworkMiddlewareService.setMiddleware.selector, address(votingPowers_)),
            bytes32(0),
            bytes32(0)
        );
        votingPowerProviders.set(block.chainid, address(votingPowers_));
        vm.stopBroadcast();

        console.log("VotingPowers:", address(votingPowers_));
        return IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(votingPowers_)});
    }

    function setupDVN() public returns (IValSetDriver.CrossChainAddress memory) {
        // On source chain, DVN doesn't need Settlement (Settlement is on dest chain)
        // We pass address(0) for settlement - it won't be used on source chain
        vm.startBroadcast(deployer);
        SymbioticLayerZeroDVN dvn = new SymbioticLayerZeroDVN(
            address(0), // Settlement not needed on source chain
            DVN_BASE_FEE
        );
        dvns.set(block.chainid, address(dvn));
        vm.stopBroadcast();

        console.log("DVN (Source):", address(dvn));
        return IValSetDriver.CrossChainAddress({chainId: uint64(block.chainid), addr: address(dvn)});
    }

    function logAndDumpSourceChainContracts() public {
        string memory obj = "sourceChainContracts";

        vm.serializeAddress(obj, "network", address(network));

        vm.serializeUint("keyRegistry", "chainId", keyRegistry.chainId);
        string memory keyRegistryData = vm.serializeAddress("keyRegistry", "addr", keyRegistry.addr);
        vm.serializeString(obj, "keyRegistry", keyRegistryData);

        (uint256 vpChainId, address vpAddr) = votingPowerProviders.at(0);
        vm.serializeUint("votingPowerProvider", "chainId", vpChainId);
        string memory vpData = vm.serializeAddress("votingPowerProvider", "addr", vpAddr);
        vm.serializeString(obj, "votingPowerProvider", vpData);

        (uint256 stkChainId, address stkAddr) = stakingTokens.at(0);
        vm.serializeUint("stakingToken", "chainId", stkChainId);
        string memory stkData = vm.serializeAddress("stakingToken", "addr", stkAddr);
        vm.serializeString(obj, "stakingToken", stkData);

        (uint256 dvnChainId, address dvnAddr) = dvns.at(0);
        vm.serializeUint("dvn", "chainId", dvnChainId);
        string memory dvnData = vm.serializeAddress("dvn", "addr", dvnAddr);
        string memory finalJson = vm.serializeString(obj, "dvn", dvnData);

        vm.writeJson(finalJson, "devnet/deploy-data/source_chain_contracts.json");
        console.log("Contracts saved to devnet/deploy-data/source_chain_contracts.json");
    }

    function addOperator(uint256 stakeAmount) public {
        Vm.Wallet memory operator = getOperator(operatorsCount);
        (BN254.G1Point memory g1Key, BN254.G2Point memory g2Key) = getBLSKeys(operator.privateKey);
        KeyRegistry keyRegistry_ = KeyRegistry(keyRegistry.addr);
        IERC20 stakingToken = IERC20(stakingTokens.get(block.chainid));
        VotingPowers votingPowers = VotingPowers(votingPowerProviders.get(block.chainid));

        vm.startBroadcast(deployer);
        payable(operator.addr).transfer(1 ether);
        stakingToken.transfer(operator.addr, stakeAmount);
        vm.stopBroadcast();

        vm.startBroadcast(operator.privateKey);

        symbioticCore.operatorRegistry.registerOperator();
        symbioticCore.operatorNetworkOptInService.optIn(address(network));
        votingPowers.registerOperator();
        IVault vault = IVault(votingPowers.getAutoDeployedVault(operator.addr));
        symbioticCore.operatorVaultOptInService.optIn(address(vault));

        stakingToken.approve(address(vault), stakeAmount);
        vault.deposit(address(stakingToken), stakeAmount);

        bytes memory keyBytes = KeyBlsBn254.wrap(g1Key).toBytes();
        bytes32 messageHash = keyRegistry_.hashTypedDataV4(
            keccak256(abi.encode(KEY_OWNERSHIP_TYPEHASH, operator.addr, keccak256(keyBytes)))
        );
        BN254.G1Point memory messageG1 = BN254.hashToG1(messageHash);
        BN254.G1Point memory sigG1 = messageG1.scalar_mul(operator.privateKey);
        keyRegistry_.setKey(KEY_TYPE_BLS_BN254.getKeyTag(15), keyBytes, abi.encode(sigG1), abi.encode(g2Key));

        vm.stopBroadcast();

        operatorsCount++;
        console.log("Operator added:", operator.addr, "stake:", stakeAmount);
    }

    function getOperator(uint256 index) public returns (Vm.Wallet memory operator) {
        operator = vm.createWallet(1e18 + index);
        vm.rememberKey(operator.privateKey);
        return operator;
    }

    function getBLSKeys(uint256 privateKey) public view returns (BN254.G1Point memory, BN254.G2Point memory) {
        BN254.G1Point memory G1Key = BN254.generatorG1().scalar_mul(privateKey);
        BN254.G2Point memory G2 = BN254.generatorG2();
        (uint256 x1, uint256 x2, uint256 y1, uint256 y2) =
            BN254G2.ECTwistMul(privateKey, G2.X[1], G2.X[0], G2.Y[1], G2.Y[0]);
        return (G1Key, BN254.G2Point([x2, x1], [y2, y1]));
    }

    function printOperatorsInfo() public view {
        VotingPowers votingPowers = VotingPowers(votingPowerProviders.get(block.chainid));
        address[] memory operators = votingPowers.getOperators();
        console.log("Total operators:", operators.length);
    }
}
