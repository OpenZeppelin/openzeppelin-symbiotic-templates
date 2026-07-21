// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { Test } from "forge-std/Test.sol";

import { Driver } from "../../src/symbiotic/Driver.sol";
import { KeyRegistry } from "../../src/symbiotic/KeyRegistry.sol";
import { Settlement } from "../../src/symbiotic/Settlement.sol";
import { VotingPowers } from "../../src/symbiotic/VotingPowers.sol";

import { INetworkManager } from "@symbioticfi/relay-contracts/interfaces/modules/base/INetworkManager.sol";
import { IOzEIP712 } from "@symbioticfi/relay-contracts/interfaces/modules/base/IOzEIP712.sol";
import { IOzOwnable } from "@symbioticfi/relay-contracts/interfaces/modules/common/permissions/IOzOwnable.sol";
import { IKeyRegistry } from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";
import { ISettlement } from "@symbioticfi/relay-contracts/interfaces/modules/settlement/ISettlement.sol";
import { IEpochManager } from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IEpochManager.sol";
import { IValSetDriver } from "@symbioticfi/relay-contracts/interfaces/modules/valset-driver/IValSetDriver.sol";
import {
    IVotingPowerProvider
} from "@symbioticfi/relay-contracts/interfaces/modules/voting-power/IVotingPowerProvider.sol";
import {
    IOpNetVaultAutoDeploy
} from "@symbioticfi/relay-contracts/interfaces/modules/voting-power/extensions/IOpNetVaultAutoDeploy.sol";

contract RegistryStub {
    function isEntity(address) external pure returns (bool) {
        return true;
    }
}

contract DelegatorStub {
    address public operator;

    constructor(address operator_) {
        operator = operator_;
    }

    function TYPE() external pure returns (uint64) {
        return 2;
    }
}

contract VaultStub {
    address public delegator;
    address public collateral;
    bool public initialized = true;
    address public slasher;
    uint48 public epochDuration = 1;

    constructor(address delegator_, address collateral_) {
        delegator = delegator_;
        collateral = collateral_;
    }

    function isInitialized() external view returns (bool) {
        return initialized;
    }
}

contract VotingPowersHarness is VotingPowers {
    constructor(
        address operatorRegistry,
        address vaultFactory,
        address vaultConfigurator
    )
        VotingPowers(operatorRegistry, vaultFactory, vaultConfigurator)
    { }

    function exposed_registerOperatorVault(address operator, address vault) external {
        _registerOperatorVaultImpl(operator, vault);
    }

    function exposed_unregisterOperatorVault(address operator, address vault) external {
        _unregisterOperatorVaultImpl(operator, vault);
    }
}

contract SymbioticWrappersTest is Test {
    function _keyRegistryInitParams() internal pure returns (IKeyRegistry.KeyRegistryInitParams memory params) {
        IOzEIP712.OzEIP712InitParams memory eip712Params =
            IOzEIP712.OzEIP712InitParams({ name: "KeyRegistry", version: "1" });
        params = IKeyRegistry.KeyRegistryInitParams({ ozEip712InitParams: eip712Params });
    }

    function _settlementInitParams() internal pure returns (ISettlement.SettlementInitParams memory params) {
        INetworkManager.NetworkManagerInitParams memory networkParams =
            INetworkManager.NetworkManagerInitParams({ network: address(0x1001), subnetworkId: 1 });
        IOzEIP712.OzEIP712InitParams memory eip712Params =
            IOzEIP712.OzEIP712InitParams({ name: "Settlement", version: "1" });
        params = ISettlement.SettlementInitParams({
            networkManagerInitParams: networkParams, ozEip712InitParams: eip712Params, sigVerifier: address(0xBEEF)
        });
    }

    function _driverInitParams() internal view returns (IValSetDriver.ValSetDriverInitParams memory params) {
        INetworkManager.NetworkManagerInitParams memory networkParams =
            INetworkManager.NetworkManagerInitParams({ network: address(0x2001), subnetworkId: 2 });
        IEpochManager.EpochManagerInitParams memory epochParams =
            IEpochManager.EpochManagerInitParams({ epochDuration: 1, epochDurationTimestamp: uint48(block.timestamp) });
        IValSetDriver.CrossChainAddress memory keysProvider =
            IValSetDriver.CrossChainAddress({ chainId: 1, addr: address(0xCAFE) });
        IValSetDriver.CrossChainAddress[] memory votingPowerProviders = new IValSetDriver.CrossChainAddress[](0);
        IValSetDriver.CrossChainAddress[] memory settlements = new IValSetDriver.CrossChainAddress[](0);
        uint8[] memory requiredKeyTags = new uint8[](1);
        requiredKeyTags[0] = 1;
        IValSetDriver.QuorumThreshold[] memory quorumThresholds = new IValSetDriver.QuorumThreshold[](0);

        params = IValSetDriver.ValSetDriverInitParams({
            networkManagerInitParams: networkParams,
            epochManagerInitParams: epochParams,
            numAggregators: 1,
            numCommitters: 1,
            committerSlotDuration: 1,
            votingPowerProviders: votingPowerProviders,
            keysProvider: keysProvider,
            settlements: settlements,
            maxVotingPower: 100,
            minInclusionVotingPower: 1,
            maxValidatorsCount: 1,
            requiredKeyTags: requiredKeyTags,
            quorumThresholds: quorumThresholds,
            requiredHeaderKeyTag: 1,
            verificationType: 0
        });
    }

    function _votingPowersInitParams()
        internal
        view
        returns (
            IVotingPowerProvider.VotingPowerProviderInitParams memory votingParams,
            IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams memory autoDeployParams,
            IOzOwnable.OzOwnableInitParams memory ownableParams
        )
    {
        INetworkManager.NetworkManagerInitParams memory networkParams =
            INetworkManager.NetworkManagerInitParams({ network: address(0x3002), subnetworkId: 3 });
        IOzEIP712.OzEIP712InitParams memory eip712Params =
            IOzEIP712.OzEIP712InitParams({ name: "VotingPowers", version: "1" });
        votingParams = IVotingPowerProvider.VotingPowerProviderInitParams({
            networkManagerInitParams: networkParams,
            ozEip712InitParams: eip712Params,
            requireSlasher: false,
            minVaultEpochDuration: 1,
            token: address(0xBEEF)
        });

        IOpNetVaultAutoDeploy.AutoDeployConfig memory config = IOpNetVaultAutoDeploy.AutoDeployConfig({
            epochDuration: 1, collateral: address(0xBEEF), burner: address(0), withSlasher: false, isBurnerHook: false
        });
        autoDeployParams = IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams({
            isAutoDeployEnabled: false, config: config, isSetMaxNetworkLimitHookEnabled: false
        });
        ownableParams = IOzOwnable.OzOwnableInitParams({ owner: address(this) });
    }

    function test_keyRegistry_initialize() public {
        KeyRegistry keyRegistry = new KeyRegistry();
        IKeyRegistry.KeyRegistryInitParams memory params = _keyRegistryInitParams();

        keyRegistry.initialize(params);

        // Verify EIP712 domain was initialized correctly
        (, string memory name, string memory version,,,,) = keyRegistry.eip712Domain();
        assertEq(name, "KeyRegistry", "EIP712 name should be set");
        assertEq(version, "1", "EIP712 version should be set");
    }

    function test_keyRegistry_initialize_revertsOnSecondCall() public {
        KeyRegistry keyRegistry = new KeyRegistry();
        IKeyRegistry.KeyRegistryInitParams memory params = _keyRegistryInitParams();

        keyRegistry.initialize(params);

        vm.expectRevert();
        keyRegistry.initialize(params);
    }

    function test_settlement_initialize_grantsAdmin_andSetsEip712Domain() public {
        Settlement settlement = new Settlement();
        address admin = makeAddr("admin");
        ISettlement.SettlementInitParams memory params = _settlementInitParams();

        settlement.initialize(params, admin);

        assertTrue(settlement.hasRole(settlement.DEFAULT_ADMIN_ROLE(), admin));
        (, string memory name, string memory version,,,,) = settlement.eip712Domain();
        assertEq(name, "Settlement");
        assertEq(version, "1");
    }

    function test_driver_initialize_grantsAdmin() public {
        Driver driver = new Driver();
        address admin = makeAddr("driverAdmin");
        IValSetDriver.ValSetDriverInitParams memory params = _driverInitParams();

        driver.initialize(params, admin);

        assertTrue(driver.hasRole(driver.DEFAULT_ADMIN_ROLE(), admin));
    }

    function test_driver_initialize_revertsOnSecondCall() public {
        Driver driver = new Driver();
        address admin = makeAddr("driverAdmin");
        IValSetDriver.ValSetDriverInitParams memory params = _driverInitParams();

        driver.initialize(params, admin);

        vm.expectRevert();
        driver.initialize(params, admin);
    }

    function test_settlement_initialize_revertsOnSecondCall() public {
        Settlement settlement = new Settlement();
        address admin = makeAddr("admin");
        ISettlement.SettlementInitParams memory params = _settlementInitParams();

        settlement.initialize(params, admin);

        vm.expectRevert();
        settlement.initialize(params, admin);
    }

    function test_votingPowers_initialize_revertsOnSecondCall() public {
        RegistryStub registry = new RegistryStub();
        address vaultConfigurator = address(0x3001);
        VotingPowersHarness votingPowers =
            new VotingPowersHarness(address(registry), address(registry), vaultConfigurator);
        (
            IVotingPowerProvider.VotingPowerProviderInitParams memory votingParams,
            IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams memory autoDeployParams,
            IOzOwnable.OzOwnableInitParams memory ownableParams
        ) = _votingPowersInitParams();

        votingPowers.initialize(votingParams, autoDeployParams, ownableParams);

        vm.expectRevert();
        votingPowers.initialize(votingParams, autoDeployParams, ownableParams);
    }

    function test_votingPowers_initialize_and_register() public {
        RegistryStub registry = new RegistryStub();
        address vaultConfigurator = address(0x3001);
        VotingPowersHarness votingPowers =
            new VotingPowersHarness(address(registry), address(registry), vaultConfigurator);
        (
            IVotingPowerProvider.VotingPowerProviderInitParams memory votingParams,
            IOpNetVaultAutoDeploy.OpNetVaultAutoDeployInitParams memory autoDeployParams,
            IOzOwnable.OzOwnableInitParams memory ownableParams
        ) = _votingPowersInitParams();

        votingPowers.initialize(votingParams, autoDeployParams, ownableParams);

        address operator = makeAddr("operator");
        DelegatorStub delegator = new DelegatorStub(operator);
        VaultStub vault = new VaultStub(address(delegator), address(0xBEEF));

        vm.prank(operator);
        votingPowers.registerOperator();

        votingPowers.exposed_registerOperatorVault(operator, address(vault));
        votingPowers.exposed_unregisterOperatorVault(operator, address(vault));
    }
}
