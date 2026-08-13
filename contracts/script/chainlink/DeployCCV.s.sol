// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {stdJson} from "forge-std/StdJson.sol";

import {CREATE2Factory} from "@chainlink/contracts-ccip/contracts/CREATE2Factory.sol";
import {VersionedVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/ccvs/VersionedVerifierResolver.sol";

import {SymbioticVerifier} from "../../src/chainlink/SymbioticVerifier.sol";
import {MockCCIPOffRamp} from "../../src/chainlink/mocks/MockCCIPOffRamp.sol";
import {MockCCIPOnRamp} from "../../src/chainlink/mocks/MockCCIPOnRamp.sol";
import {MockRMN} from "../../src/chainlink/mocks/MockRMN.sol";
import {MockRouter} from "../../src/chainlink/mocks/MockRouter.sol";
import {NoOpSettlement} from "../../src/mocks/NoOpSettlement.sol";

/// @title DeployCCV
/// @notice Deploys the factory, stable resolver, and chain-specific Symbiotic verifier topology.
contract DeployCCV is Script {
    using stdJson for string;

    bytes4 internal constant VERSION_TAG_V1_0_0 = 0x1a75bd93;
    bytes32 public constant RESOLVER_SALT = keccak256("symbiotic.ccv.versioned-verifier-resolver.v1");

    string internal constant RESOLVER_BYTECODE_PATH =
        "node_modules/@chainlink/contracts-ccip/bytecode/v2_0_0/versioned_verifier_resolver.bin";
    string internal constant DEPLOY_DATA_DIR = "deploy-data/chainlink";
    string internal constant FACTORY_DATA_PATH = "deploy-data/chainlink/ccv_factory.json";
    string internal constant RESOLVER_DATA_PATH = "deploy-data/chainlink/ccv_resolver.json";

    struct DeploymentRecord {
        address resolver;
        address verifier;
        address router;
        address rmn;
        address settlement;
        address onRamp;
        address offRamp;
        address factory;
    }

    function deployFactory(address[] memory allowList) external returns (address factoryAddress) {
        address factoryDeployer = vm.envAddress("CCV_FACTORY_DEPLOYER");
        require(vm.getNonce(factoryDeployer) == 0, "CCV_FACTORY_DEPLOYER nonce must be zero");

        vm.startBroadcast(factoryDeployer);
        CREATE2Factory factory = new CREATE2Factory(allowList);
        vm.stopBroadcast();

        factoryAddress = address(factory);
        _saveFactory(factoryAddress, factoryDeployer);
        console.log("CREATE2Factory:", factoryAddress);
    }

    /// @dev For an EOA `resolverOwner` whose key is available to the forge run, broadcasts
    ///      `acceptOwnership()` directly. A contract owner (Safe/timelock) cannot sign here,
    ///      so the accept step is skipped and the pending call is printed instead — execute it
    ///      from the owner (see `printAcceptOwnershipCall`).
    function deployResolver(address resolverOwner) external returns (address resolverAddress) {
        require(resolverOwner != address(0), "resolver owner required");
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");
        CREATE2Factory factory = CREATE2Factory(_readAddress(FACTORY_DATA_PATH, ".factory"));
        bytes memory creationCode = _resolverCreationCode();
        address predicted = factory.computeAddress(creationCode, RESOLVER_SALT);

        vm.startBroadcast(deployer);
        resolverAddress = factory.createAndTransferOwnership(creationCode, RESOLVER_SALT, resolverOwner);
        vm.stopBroadcast();
        require(resolverAddress == predicted, "resolver address mismatch");

        if (resolverOwner.code.length == 0) {
            vm.startBroadcast(resolverOwner);
            VersionedVerifierResolver(resolverAddress).acceptOwnership();
            vm.stopBroadcast();
        } else {
            console.log("resolver owner is a contract; skipping acceptOwnership broadcast");
            _printCall(
                "acceptOwnership (execute from the resolver owner)",
                resolverAddress,
                abi.encodeWithSignature("acceptOwnership()")
            );
        }

        _saveResolver(resolverAddress, address(factory), resolverOwner);
        console.log("VersionedVerifierResolver:", resolverAddress);
    }

    function deployVerifier(
        address settlement,
        address rmn,
        bytes4 versionTag
    ) external returns (address verifierAddress) {
        require(settlement != address(0), "settlement address required");
        require(rmn != address(0), "rmn address required");
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");
        string[] memory storageLocations = _storageLocations();
        (uint256 maxEpochValidity, uint256 epochValidity) = _epochValidityParams();

        vm.startBroadcast(deployer);
        verifierAddress = address(
            new SymbioticVerifier(
                settlement, storageLocations, rmn, versionTag, maxEpochValidity, epochValidity
            )
        );
        vm.stopBroadcast();

        string memory deploymentRole = vm.envOr("CCV_DEPLOYMENT_ROLE", string(""));
        if (bytes(deploymentRole).length != 0) {
            bool source = _isSourceRole(deploymentRole);
            DeploymentRecord memory deployment = _readContracts(source);
            require(deployment.rmn == rmn, "rmn does not match deployment role");
            deployment.settlement = settlement;
            deployment.verifier = verifierAddress;
            _saveContracts(source, deployment);
        }

        console.log("SymbioticVerifier:", verifierAddress);
    }

    function registerVerifier(
        address resolverAddress,
        bytes4 versionTag,
        address verifierAddress,
        uint64[] memory destChainSelectors
    ) external {
        require(resolverAddress != address(0), "resolver address required");
        require(verifierAddress != address(0), "verifier address required");
        address deployer = vm.envOr("CCV_RESOLVER_OWNER", vm.envAddress("DEPLOYER_ADDRESS"));

        vm.startBroadcast(deployer);
        _registerVerifier(VersionedVerifierResolver(resolverAddress), versionTag, verifierAddress, destChainSelectors);
        vm.stopBroadcast();
    }

    /// @notice Deploys the local router, RMN, and resolver-aware ramps for one chain role.
    /// @dev The generic factory/resolver artifacts are the hand-off from the preceding split deploy steps.
    function deployLocalMocks(uint64 remoteChainSelector) external {
        require(remoteChainSelector != 0, "remote chain selector required");
        bool source = _isSourceRole(vm.envString("CCV_DEPLOYMENT_ROLE"));
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        // Merge into any existing record so a previously persisted verifier/settlement survives.
        DeploymentRecord memory deployment;
        if (vm.exists(_contractsPath(source))) {
            deployment = _readContracts(source);
        }
        deployment.factory = _readAddress(RESOLVER_DATA_PATH, ".factory");
        deployment.resolver = _readAddress(RESOLVER_DATA_PATH, ".resolver");

        vm.startBroadcast(deployer);
        MockRouter router = new MockRouter();
        MockRMN rmn = new MockRMN();
        MockCCIPOnRamp onRamp = new MockCCIPOnRamp(deployment.resolver);
        MockCCIPOffRamp offRamp = new MockCCIPOffRamp(remoteChainSelector);
        router.setOnRamp(remoteChainSelector, address(onRamp));
        router.setOffRamp(remoteChainSelector, address(offRamp), true);
        vm.stopBroadcast();

        deployment.router = address(router);
        deployment.rmn = address(rmn);
        deployment.onRamp = address(onRamp);
        deployment.offRamp = address(offRamp);
        _saveContracts(source, deployment);
    }

    /// @notice Testnet source-chain flow using existing factory/resolver artifacts and real CCIP Router/RMN.
    function deploySourceCcvOnly(
        address settlement,
        address router,
        address rmn,
        address onRamp,
        address offRamp
    ) external {
        _deployCcvOnly(settlement, router, rmn, onRamp, offRamp, true);
    }

    /// @notice Testnet destination-chain flow using existing factory/resolver artifacts and real CCIP Router/RMN.
    function deployDestCcvOnly(
        address settlement,
        address router,
        address rmn,
        address onRamp,
        address offRamp
    ) external {
        _deployCcvOnly(settlement, router, rmn, onRamp, offRamp, false);
    }

    function deployNoOpSettlement() external {
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");

        vm.startBroadcast(deployer);
        NoOpSettlement noOp = new NoOpSettlement();
        vm.stopBroadcast();

        string memory objectKey = "noOpSettlement";
        vm.serializeUint(objectKey, "chainId", block.chainid);
        string memory json = vm.serializeAddress(objectKey, "settlement", address(noOp));
        vm.createDir(DEPLOY_DATA_DIR, true);
        vm.writeJson(json, "deploy-data/chainlink/noop_settlement.json");
        console.log("NoOpSettlement:", address(noOp));
    }

    function _deployCcvOnly(
        address settlement,
        address router,
        address rmn,
        address onRamp,
        address offRamp,
        bool source
    ) internal {
        require(settlement != address(0), "settlement address required");
        require(router != address(0), "router address required");
        require(rmn != address(0), "rmn address required");
        require(onRamp != address(0), "onRamp address required");
        require(offRamp != address(0), "offRamp address required");

        DeploymentRecord memory deployment;
        deployment.resolver = _readAddress(RESOLVER_DATA_PATH, ".resolver");
        deployment.factory = _readAddress(RESOLVER_DATA_PATH, ".factory");
        deployment.router = router;
        deployment.rmn = rmn;
        deployment.settlement = settlement;
        deployment.onRamp = onRamp;
        deployment.offRamp = offRamp;

        uint64 remoteChainSelector = uint64(vm.envUint("CCV_REMOTE_CHAIN_SELECTOR"));
        address deployer = vm.envAddress("DEPLOYER_ADDRESS");
        address resolverOwner = vm.envOr("CCV_RESOLVER_OWNER", deployer);

        (uint256 maxEpochValidity, uint256 epochValidity) = _epochValidityParams();

        vm.startBroadcast(deployer);
        deployment.verifier = address(
            new SymbioticVerifier(
                settlement, _storageLocations(), rmn, VERSION_TAG_V1_0_0, maxEpochValidity, epochValidity
            )
        );
        vm.stopBroadcast();

        uint64[] memory selectors = new uint64[](1);
        selectors[0] = remoteChainSelector;
        vm.startBroadcast(resolverOwner);
        _registerVerifier(
            VersionedVerifierResolver(deployment.resolver), VERSION_TAG_V1_0_0, deployment.verifier, selectors
        );
        vm.stopBroadcast();

        _saveContracts(source, deployment);
    }

    function _registerVerifier(
        VersionedVerifierResolver resolver,
        bytes4 versionTag,
        address verifier,
        uint64[] memory destChainSelectors
    ) internal {
        resolver.applyInboundImplementationUpdates(_inboundArgs(versionTag, verifier));
        resolver.applyOutboundImplementationUpdates(_outboundArgs(verifier, destChainSelectors));
    }

    // ============ Governance calldata helpers ============
    // Non-broadcasting: print (target, calldata) for the owner-gated calls so a
    // Safe/timelock owner can execute them without rebuilding these scripts.

    /// @notice Prints the call for the pending owner to accept resolver ownership.
    function printAcceptOwnershipCall(address resolverAddress)
        external
        view
        returns (address target, bytes memory data)
    {
        target = resolverAddress;
        data = abi.encodeWithSignature("acceptOwnership()");
        _printCall("acceptOwnership (execute from the pending owner)", target, data);
    }

    /// @notice Prints the two calls that register a verifier version on the resolver.
    function printRegisterVerifierCalls(
        address resolverAddress,
        bytes4 versionTag,
        address verifierAddress,
        uint64[] memory destChainSelectors
    ) external view returns (address target, bytes memory inboundData, bytes memory outboundData) {
        target = resolverAddress;
        inboundData = abi.encodeCall(
            VersionedVerifierResolver.applyInboundImplementationUpdates,
            (_inboundArgs(versionTag, verifierAddress))
        );
        outboundData = abi.encodeCall(
            VersionedVerifierResolver.applyOutboundImplementationUpdates,
            (_outboundArgs(verifierAddress, destChainSelectors))
        );
        _printCall("applyInboundImplementationUpdates (execute from the resolver owner)", target, inboundData);
        _printCall("applyOutboundImplementationUpdates (execute from the resolver owner)", target, outboundData);
    }

    /// @notice Prints the call that sets the verifier's epoch validity window.
    function printSetEpochValidityCall(address verifierAddress, uint256 epochValidity)
        external
        view
        returns (address target, bytes memory data)
    {
        target = verifierAddress;
        data = abi.encodeCall(SymbioticVerifier.setEpochValidity, (epochValidity));
        _printCall("setEpochValidity (execute from the verifier owner)", target, data);
    }

    function _printCall(string memory label, address target, bytes memory data) internal view {
        console.log(label);
        console.log("  target:", target);
        console.log("  calldata:");
        console.logBytes(data);
    }

    function _inboundArgs(bytes4 versionTag, address verifier)
        internal
        pure
        returns (VersionedVerifierResolver.InboundImplementationArgs[] memory inbound)
    {
        inbound = new VersionedVerifierResolver.InboundImplementationArgs[](1);
        inbound[0] = VersionedVerifierResolver.InboundImplementationArgs({
            version: versionTag, verifier: verifier
        });
    }

    function _outboundArgs(address verifier, uint64[] memory destChainSelectors)
        internal
        pure
        returns (VersionedVerifierResolver.OutboundImplementationArgs[] memory outbound)
    {
        outbound = new VersionedVerifierResolver.OutboundImplementationArgs[](destChainSelectors.length);
        for (uint256 i = 0; i < destChainSelectors.length; ++i) {
            outbound[i] = VersionedVerifierResolver.OutboundImplementationArgs({
                destChainSelector: destChainSelectors[i], verifier: verifier
            });
        }
    }

    function _storageLocations() internal view returns (string[] memory locations) {
        string memory rawLocations = vm.trim(vm.envOr("CCV_STORAGE_LOCATION_URIS", string("")));
        require(bytes(rawLocations).length != 0, "CCV_STORAGE_LOCATION_URIS is required");

        locations = vm.split(rawLocations, ",");
        for (uint256 i = 0; i < locations.length; ++i) {
            locations[i] = vm.trim(locations[i]);
            require(bytes(locations[i]).length != 0, "CCV_STORAGE_LOCATION_URIS contains an empty URI");
        }
    }

    /// @dev The verifier's epoch validity ceiling must not exceed the Symbiotic slashing
    /// window: a proof must only verify while the attesting stake is still slashable.
    function _epochValidityParams()
        internal
        view
        returns (uint256 maxEpochValidity, uint256 epochValidity)
    {
        maxEpochValidity = vm.envOr("SLASHING_WINDOW", uint256(0));
        require(
            maxEpochValidity != 0,
            "SLASHING_WINDOW (seconds) is required: the epoch validity ceiling must match the deployment's Symbiotic slashing window"
        );
        epochValidity = vm.envOr("CCV_EPOCH_VALIDITY", maxEpochValidity);
        require(
            epochValidity != 0 && epochValidity <= maxEpochValidity,
            "CCV_EPOCH_VALIDITY must be in (0, SLASHING_WINDOW]"
        );
    }

    function _resolverCreationCode() internal view returns (bytes memory) {
        return vm.parseBytes(vm.trim(vm.readFile(RESOLVER_BYTECODE_PATH)));
    }

    function _readAddress(string memory path, string memory key) internal view returns (address) {
        return vm.readFile(path).readAddress(key);
    }

    function _saveFactory(address factory, address deployer) internal {
        string memory objectKey = "ccvFactory";
        vm.serializeUint(objectKey, "chainId", block.chainid);
        vm.serializeAddress(objectKey, "deployer", deployer);
        string memory json = vm.serializeAddress(objectKey, "factory", factory);
        vm.createDir(DEPLOY_DATA_DIR, true);
        vm.writeJson(json, FACTORY_DATA_PATH);
    }

    function _saveResolver(address resolver, address factory, address resolverOwner) internal {
        string memory objectKey = "ccvResolver";
        vm.serializeUint(objectKey, "chainId", block.chainid);
        vm.serializeAddress(objectKey, "factory", factory);
        vm.serializeAddress(objectKey, "resolverOwner", resolverOwner);
        vm.serializeBytes32(objectKey, "salt", RESOLVER_SALT);
        string memory json = vm.serializeAddress(objectKey, "resolver", resolver);
        vm.createDir(DEPLOY_DATA_DIR, true);
        vm.writeJson(json, RESOLVER_DATA_PATH);
    }

    function _saveContracts(bool source, DeploymentRecord memory deployment) internal {
        string memory objectKey = source ? "sourceCCV" : "destCCV";
        vm.serializeUint(objectKey, "chainId", block.chainid);
        vm.serializeAddress(objectKey, "factory", deployment.factory);
        vm.serializeAddress(objectKey, "resolver", deployment.resolver);
        vm.serializeAddress(objectKey, "verifier", deployment.verifier);
        vm.serializeAddress(objectKey, "router", deployment.router);
        vm.serializeAddress(objectKey, "rmn", deployment.rmn);
        vm.serializeAddress(objectKey, "settlement", deployment.settlement);
        vm.serializeAddress(objectKey, "onRamp", deployment.onRamp);
        string memory json = vm.serializeAddress(objectKey, "offRamp", deployment.offRamp);
        vm.createDir(DEPLOY_DATA_DIR, true);
        vm.writeJson(
            json,
            _contractsPath(source)
        );
    }

    function _readContracts(bool source) internal view returns (DeploymentRecord memory deployment) {
        string memory path = _contractsPath(source);
        deployment.factory = _readAddress(path, ".factory");
        deployment.resolver = _readAddress(path, ".resolver");
        deployment.verifier = _readAddress(path, ".verifier");
        deployment.router = _readAddress(path, ".router");
        deployment.rmn = _readAddress(path, ".rmn");
        deployment.settlement = _readAddress(path, ".settlement");
        deployment.onRamp = _readAddress(path, ".onRamp");
        deployment.offRamp = _readAddress(path, ".offRamp");
    }

    function _contractsPath(bool source) internal pure returns (string memory) {
        return source
            ? "deploy-data/chainlink/ccv_source_contracts.json"
            : "deploy-data/chainlink/ccv_dest_contracts.json";
    }

    function _isSourceRole(string memory deploymentRole) internal pure returns (bool) {
        bytes32 roleHash = keccak256(bytes(deploymentRole));
        if (roleHash == keccak256("source")) {
            return true;
        }
        require(roleHash == keccak256("destination"), "CCV_DEPLOYMENT_ROLE must be source or destination");
        return false;
    }
}
