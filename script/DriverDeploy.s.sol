// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";

import {INetworkManager} from "@symbioticfi/relay-contracts/interfaces/modules/base/INetworkManager.sol";
import {IEpochManager} from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IEpochManager.sol";
import {IValSetDriver} from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IValSetDriver.sol";

import {Driver} from "../src/symbiotic/Driver.sol";

/// @title DriverDeploy
/// @notice Phase 3: Deploy Driver on source chain (after all Settlements are deployed)
/// @dev Driver manages validator sets and commits them to all Settlements
contract DriverDeploy is Script {
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

    // NOTE: Fields MUST be in alphabetical order for Foundry's parseJson/abi.decode
    struct DestChainContracts {
        CrossChainAddress dvn;
        CrossChainAddress settlement;
    }

    uint48 internal immutable EPOCH_DURATION = uint48(vm.envOr("EPOCH_TIME", uint256(60)));
    uint208 internal constant MAX_VALIDATORS_COUNT = 1000;
    uint256 internal constant MAX_VOTING_POWER = 2 ** 247;
    uint256 internal constant MIN_INCLUSION_VOTING_POWER = 0;
    uint248 internal constant QUORUM_THRESHOLD = (uint248(1e18) * 2) / 3 + 1; // 2/3 + 1
    uint8 internal constant REQUIRED_KEY_TAG = 15; // BLS-BN254/15
    uint208 internal immutable NUM_AGGREGATORS = uint208(vm.envOr("NUM_AGGREGATORS", uint256(1)));
    uint208 internal immutable NUM_COMMITTERS = uint208(vm.envOr("NUM_COMMITTERS", uint256(1)));

    address internal deployer;
    Driver internal driver;

    SourceChainContracts internal sourceContracts;
    DestChainContracts internal destContracts;

    function getDeployerAddress() internal view returns (address) {
        return vm.envOr("DEPLOYER_ADDRESS", 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266);
    }

    function run() public {
        deployer = getDeployerAddress();

        console.log("=== Phase 3: Driver Deployment ===");
        console.log("Chain ID:", block.chainid);

        loadSourceChainContracts();
        loadDestChainContracts();

        setupDriver();

        logAndDumpDriverContracts();

        console.log("");
        console.log("=== Driver Deployment Complete ===");
        console.log("All contracts deployed! Ready for off-chain operator setup.");
    }

    function loadSourceChainContracts() internal {
        string memory root = vm.projectRoot();
        string memory path = string.concat(root, "/devnet/deploy-data/source_chain_contracts.json");

        if (!vm.exists(path)) {
            console.log("ERROR: source_chain_contracts.json not found");
            console.log("Run SourceChainDeploy.s.sol first");
            revert("Missing source chain contracts");
        }

        string memory json = vm.readFile(path);
        bytes memory data = vm.parseJson(json);
        sourceContracts = abi.decode(data, (SourceChainContracts));

        console.log("Loaded source chain contracts:");
        console.log("   Network:", sourceContracts.network);
        console.log("   KeyRegistry:", sourceContracts.keyRegistry.addr);
        console.log("   VotingPowers:", sourceContracts.votingPowerProvider.addr);
    }

    function loadDestChainContracts() internal {
        string memory root = vm.projectRoot();
        string memory path = string.concat(root, "/devnet/deploy-data/dest_chain_contracts.json");

        if (!vm.exists(path)) {
            console.log("ERROR: dest_chain_contracts.json not found");
            console.log("Run DestinationChainDeploy.s.sol first");
            revert("Missing dest chain contracts");
        }

        string memory json = vm.readFile(path);
        bytes memory data = vm.parseJson(json);
        destContracts = abi.decode(data, (DestChainContracts));

        console.log("Loaded dest chain contracts:");
        console.log("   Settlement:", destContracts.settlement.addr);
        console.log("   DVN:", destContracts.dvn.addr);
    }

    function setupDriver() public returns (address) {
        vm.startBroadcast(deployer);

        driver = new Driver{salt: "Driver"}();

        // VotingPowerProviders (source chain only for now)
        IValSetDriver.CrossChainAddress[] memory votingPowerProviders_ = new IValSetDriver.CrossChainAddress[](1);
        votingPowerProviders_[0] = IValSetDriver.CrossChainAddress({
            chainId: sourceContracts.votingPowerProvider.chainId,
            addr: sourceContracts.votingPowerProvider.addr
        });

        // Settlements (destination chain for now, can add more)
        IValSetDriver.CrossChainAddress[] memory settlements_ = new IValSetDriver.CrossChainAddress[](1);
        settlements_[0] = IValSetDriver.CrossChainAddress({
            chainId: destContracts.settlement.chainId,
            addr: destContracts.settlement.addr
        });

        // Quorum thresholds
        IValSetDriver.QuorumThreshold[] memory quorumThresholds = new IValSetDriver.QuorumThreshold[](1);
        quorumThresholds[0] = IValSetDriver.QuorumThreshold({
            keyTag: REQUIRED_KEY_TAG,
            quorumThreshold: QUORUM_THRESHOLD
        });

        uint8[] memory requiredKeyTags = new uint8[](1);
        requiredKeyTags[0] = REQUIRED_KEY_TAG;

        driver.initialize(
            IValSetDriver.ValSetDriverInitParams({
                networkManagerInitParams: INetworkManager.NetworkManagerInitParams({
                    network: sourceContracts.network,
                    subnetworkId: 0
                }),
                epochManagerInitParams: IEpochManager.EpochManagerInitParams({
                    epochDuration: EPOCH_DURATION,
                    epochDurationTimestamp: 0
                }),
                numAggregators: NUM_AGGREGATORS,
                numCommitters: NUM_COMMITTERS,
                votingPowerProviders: votingPowerProviders_,
                keysProvider: IValSetDriver.CrossChainAddress({
                    chainId: sourceContracts.keyRegistry.chainId,
                    addr: sourceContracts.keyRegistry.addr
                }),
                settlements: settlements_,
                maxVotingPower: MAX_VOTING_POWER,
                minInclusionVotingPower: MIN_INCLUSION_VOTING_POWER,
                maxValidatorsCount: MAX_VALIDATORS_COUNT,
                requiredKeyTags: requiredKeyTags,
                quorumThresholds: quorumThresholds,
                requiredHeaderKeyTag: REQUIRED_KEY_TAG,
                verificationType: 1 // Simple BLS verification
            }),
            deployer
        );

        vm.stopBroadcast();

        console.log("Driver:", address(driver));
        return address(driver);
    }

    function logAndDumpDriverContracts() public {
        string memory obj = "driverContracts";

        vm.serializeUint("driver", "chainId", block.chainid);
        string memory driverData = vm.serializeAddress("driver", "addr", address(driver));
        string memory finalJson = vm.serializeString(obj, "driver", driverData);

        vm.writeJson(finalJson, "devnet/deploy-data/driver_contracts.json");
        console.log("Contracts saved to devnet/deploy-data/driver_contracts.json");
    }
}
