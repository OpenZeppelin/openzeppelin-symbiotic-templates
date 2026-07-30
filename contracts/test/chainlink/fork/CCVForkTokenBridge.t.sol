// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

import { IRouter } from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";
import { ITokenAdminRegistry } from "@chainlink/contracts-ccip/contracts/interfaces/ITokenAdminRegistry.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";
import { Pool } from "@chainlink/contracts-ccip/contracts/libraries/Pool.sol";
import { RateLimiter } from "@chainlink/contracts-ccip/contracts/libraries/RateLimiter.sol";
import { OnRamp } from "@chainlink/contracts-ccip/contracts/onRamp/OnRamp.sol";
import { IBurnMintERC20 } from "@chainlink/contracts-ccip/contracts/interfaces/IBurnMintERC20.sol";
import { BurnMintTokenPool } from "@chainlink/contracts-ccip/contracts/pools/BurnMintTokenPool.sol";
import { TokenPool } from "@chainlink/contracts-ccip/contracts/pools/TokenPool.sol";
import { BaseERC20 } from "@chainlink/contracts-ccip/contracts/tokens/BaseERC20.sol";
import { CrossChainToken } from "@chainlink/contracts-ccip/contracts/tokens/CrossChainToken.sol";

import { CcipExtraArgs } from "../../../src/chainlink/CcipExtraArgs.sol";
import { CCVForkBase } from "./CCVForkBase.sol";

interface IHooksAdmin {
    struct AuthorizedCallerArgs {
        address[] addedCallers;
        address[] removedCallers;
    }

    struct CCVConfigArg {
        uint64 remoteChainSelector;
        address[] outboundCCVs;
        address[] thresholdOutboundCCVs;
        address[] inboundCCVs;
        address[] thresholdInboundCCVs;
    }

    function applyAuthorizedCallerUpdates(AuthorizedCallerArgs memory) external;
    function applyCCVConfigUpdates(CCVConfigArg[] calldata) external;
}

interface IOwnable {
    function owner() external view returns (address);
}

/// @notice Shared scaffolding for token-bridge fork tests: deploys our production-faithful token
/// pool (CrossChainToken + BurnMintTokenPool + AdvancedPoolHooks that mandate the Symbiotic
/// resolver), self-registers the token with the real TokenAdminRegistry, and wires one lane.
/// Chainlink's Committee verifier RESOLVER is the same address on both chains (inert below 10 SYMB).
abstract contract TokenBridgeForkFixture is CCVForkBase {
    address internal constant COMMITTEE_RESOLVER = 0xFCCfCd7aF7c98fe9233d39CA8C118C35D53eFbE5;
    string internal constant HOOKS_BYTECODE_PATH =
        "node_modules/@chainlink/contracts-ccip/bytecode/v2_0_0/advanced_pool_hooks.bin";
    uint8 internal constant DECIMALS = 18;
    uint256 internal constant PRE_MINT = 1_000_000 ether;
    uint256 internal constant THRESHOLD = 10 ether;

    CrossChainToken internal token;
    BurnMintTokenPool internal pool;
    address internal hooks;
    address internal rmn;
    ITokenAdminRegistry internal registry;

    /// @dev Deploys token + hooks + pool, grants roles, authorizes the pool on the hooks, and
    /// mandates our resolver on `laneSelector` (Committee resolver above the 10 SYMB threshold).
    function _deployTokenPoolAndHooks(address router, uint64 laneSelector) internal {
        token = new CrossChainToken(
            BaseERC20.ConstructorParams({
                name: "Symbiotic Bridged Token",
                symbol: "SYMB",
                maxSupply: 0, // unlimited
                preMint: PRE_MINT,
                preMintRecipient: address(this),
                decimals: DECIMALS,
                ccipAdmin: address(this)
            }),
            address(this), // burnMintRoleAdmin
            address(this) // owner
        );

        // AdvancedPoolHooks deploys from published creation bytecode (it does not compile under the
        // repo's remappings); ctor args = (allowlist, thresholdAmount, policyEngine, authorizedCallers).
        address[] memory empty = new address[](0);
        bytes memory creationCode = abi.encodePacked(
            vm.parseBytes(vm.trim(vm.readFile(HOOKS_BYTECODE_PATH))), abi.encode(empty, THRESHOLD, address(0), empty)
        );
        address deployed;
        assembly {
            deployed := create(0, add(creationCode, 0x20), mload(creationCode))
        }
        require(deployed != address(0), "hooks deploy failed");
        hooks = deployed;

        pool = new BurnMintTokenPool(IBurnMintERC20(address(token)), DECIMALS, hooks, rmn, router);
        token.grantMintAndBurnRoles(address(pool));

        address[] memory added = new address[](1);
        added[0] = address(pool);
        IHooksAdmin(hooks)
            .applyAuthorizedCallerUpdates(
                IHooksAdmin.AuthorizedCallerArgs({ addedCallers: added, removedCallers: new address[](0) })
            );

        address[] memory base = new address[](1);
        base[0] = address(resolver);
        address[] memory thresh = new address[](1);
        thresh[0] = COMMITTEE_RESOLVER;
        IHooksAdmin.CCVConfigArg[] memory cfg = new IHooksAdmin.CCVConfigArg[](1);
        cfg[0] = IHooksAdmin.CCVConfigArg({
            remoteChainSelector: laneSelector,
            outboundCCVs: base,
            thresholdOutboundCCVs: thresh,
            inboundCCVs: base,
            thresholdInboundCCVs: thresh
        });
        IHooksAdmin(hooks).applyCCVConfigUpdates(cfg);
    }

    /// @dev On the live testnet the deploy script self-registers via
    /// RegistryModuleOwnerCustom.registerAdminViaGetCCIPAdmin; on a fork we impersonate the registry
    /// owner (also authorized for proposeAdministrator) so the test needs no module address.
    function _registerTokenWithRegistry() internal {
        address regOwner = IOwnable(address(registry)).owner();
        vm.prank(regOwner);
        registry.proposeAdministrator(address(token), address(this));
        registry.acceptAdminRole(address(token));
        registry.setPool(address(token), address(pool));
        assertEq(registry.getPool(address(token)), address(pool), "pool not registered");
    }

    function _wireLane(uint64 laneSelector, address remotePool, address remoteToken) internal {
        bytes[] memory remotePools = new bytes[](1);
        remotePools[0] = abi.encode(remotePool);
        RateLimiter.Config memory off = RateLimiter.Config({ isEnabled: false, capacity: 0, rate: 0 });
        TokenPool.ChainUpdate[] memory add = new TokenPool.ChainUpdate[](1);
        add[0] = TokenPool.ChainUpdate({
            remoteChainSelector: laneSelector,
            remotePoolAddresses: remotePools,
            remoteTokenAddress: abi.encode(remoteToken),
            outboundRateLimiterConfig: off,
            inboundRateLimiterConfig: off
        });
        pool.applyChainUpdates(new uint64[](0), add);
    }

    // Receive native (ccipSend fee handling).
    receive() external payable { }
}

/// @notice Source-side token bridge against the real Base Sepolia CCIP v2 staging deployment.
/// Proves a real Router.ccipSend carrying 1 SYMB routes through our pool and BURNS it, with our CCV
/// attached. Run:  forge test --fork-url $SOURCE_RPC_URL --match-contract CCVForkTokenBridgeSource -vv
contract CCVForkTokenBridgeSourceTest is TokenBridgeForkFixture {
    address constant ROUTER = 0x0Ec6D443B425982f1F2862Dd0ffBFD431FCb6b8b;
    address constant ON_RAMP = 0x829F4e6E2B979a4B87Ecf493BE94e25087aa0Fcd;

    uint64 constant SEPOLIA_SELECTOR = 16_015_286_601_757_825_753;

    function setUp() public {
        require(block.chainid == 84_532, "expected Base Sepolia fork (chainid 84532)");

        OnRamp.StaticConfig memory sc = OnRamp(ON_RAMP).getStaticConfig();
        rmn = address(sc.rmnRemote);
        registry = ITokenAdminRegistry(sc.tokenAdminRegistry);

        _deployVerifierAndResolver(rmn, IRouter(ROUTER), SEPOLIA_SELECTOR);
        _registerOutbound(SEPOLIA_SELECTOR);

        _deployTokenPoolAndHooks(ROUTER, SEPOLIA_SELECTOR);
        _registerTokenWithRegistry();
        _wireLane(SEPOLIA_SELECTOR, makeAddr("remotePool"), makeAddr("remoteToken"));
    }

    function testTokenBridgeSetup() public view {
        assertGt(ON_RAMP.code.length, 0, "OnRamp has no code");
        assertGt(address(pool).code.length, 0, "pool has no code");
        assertGt(hooks.code.length, 0, "hooks has no code");
        assertEq(token.balanceOf(address(this)), PRE_MINT, "preMint missing");
        assertEq(registry.getPool(address(token)), address(pool), "token not registered to pool");
    }

    function testCcipSendBurnsThroughOurPool() public {
        uint256 amount = 1 ether; // below the 10 SYMB Committee threshold

        Client.EVMTokenAmount[] memory tokenAmounts = new Client.EVMTokenAmount[](1);
        tokenAmounts[0] = Client.EVMTokenAmount({ token: address(token), amount: amount });
        Client.EVM2AnyMessage memory message = Client.EVM2AnyMessage({
            receiver: abi.encode(makeAddr("destReceiver")),
            data: "",
            tokenAmounts: tokenAmounts,
            feeToken: address(0), // native
            extraArgs: CcipExtraArgs.encodeWithCcv(address(resolver), address(0), 200_000)
        });

        uint256 fee = IRouterClient(ROUTER).getFee(SEPOLIA_SELECTOR, message);
        assertGt(fee, 0, "Router quoted zero fee");

        uint256 supplyBefore = token.totalSupply();
        uint256 balBefore = token.balanceOf(address(this));

        token.approve(ROUTER, amount);
        vm.deal(address(this), fee * 2);
        bytes32 messageId = IRouterClient(ROUTER).ccipSend{ value: fee }(SEPOLIA_SELECTOR, message);

        assertTrue(messageId != bytes32(0), "ccipSend returned zero messageId");
        assertEq(token.balanceOf(address(this)), balBefore - amount, "sender not debited");
        assertEq(token.totalSupply(), supplyBefore - amount, "supply not burned");
        assertEq(token.balanceOf(address(pool)), 0, "pool retained tokens (should burn)");
    }
}

/// @notice Destination-side token bridge against the real Sepolia CCIP v2 staging deployment.
/// Proves the real OffRamp calling our pool's releaseOrMint MINTS the token to the receiver.
/// Run:  forge test --fork-url $DEST_RPC_URL --match-contract CCVForkTokenBridgeDest -vv
contract CCVForkTokenBridgeDestTest is TokenBridgeForkFixture {
    address constant ROUTER = 0x784d49a71BB4C48eB7dA4cD7e6Ecb424f9b5EAB1;
    address constant ON_RAMP = 0xA94E45744553F4B2bea9DfB8979a02962B980732;
    address constant OFF_RAMP = 0x386577d8350D5814198974d16c3C756a638fBd62; // registered for the Base Sepolia lane

    uint64 constant BASE_SEPOLIA_SELECTOR = 10_344_971_235_874_465_080;

    address internal sourcePool;

    function setUp() public {
        require(block.chainid == 11_155_111, "expected Sepolia fork (chainid 11155111)");

        OnRamp.StaticConfig memory sc = OnRamp(ON_RAMP).getStaticConfig();
        rmn = address(sc.rmnRemote);
        registry = ITokenAdminRegistry(sc.tokenAdminRegistry);

        // Inbound resolver toward the source (Base Sepolia) chain.
        _deployVerifierAndResolver(rmn, IRouter(ROUTER), BASE_SEPOLIA_SELECTOR);

        sourcePool = makeAddr("sourcePool");
        _deployTokenPoolAndHooks(ROUTER, BASE_SEPOLIA_SELECTOR);
        _registerTokenWithRegistry();
        _wireLane(BASE_SEPOLIA_SELECTOR, sourcePool, makeAddr("remoteToken"));
    }

    function testOffRampIsRegisteredForLane() public view {
        assertTrue(IRouter(ROUTER).isOffRamp(BASE_SEPOLIA_SELECTOR, OFF_RAMP), "offRamp not registered for lane");
    }

    function testReleaseOrMintMintsThroughOurPool() public {
        uint256 amount = 1 ether;
        address recipient = makeAddr("destRecipient");

        Pool.ReleaseOrMintInV1 memory input = Pool.ReleaseOrMintInV1({
            originalSender: abi.encode(makeAddr("srcSender")),
            remoteChainSelector: BASE_SEPOLIA_SELECTOR,
            receiver: recipient,
            sourceDenominatedAmount: amount,
            localToken: address(token),
            sourcePoolAddress: abi.encode(sourcePool),
            sourcePoolData: "", // empty => falls back to local decimals (18)
            offchainTokenData: ""
        });

        uint256 supplyBefore = token.totalSupply();

        vm.prank(OFF_RAMP);
        pool.releaseOrMint(input);

        assertEq(token.balanceOf(recipient), amount, "recipient not minted");
        assertEq(token.totalSupply(), supplyBefore + amount, "supply not increased");
    }
}
