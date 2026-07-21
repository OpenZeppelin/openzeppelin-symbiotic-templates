// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test} from "forge-std/Test.sol";

import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {VersionedVerifierResolver} from
    "@chainlink/contracts-ccip/contracts/ccvs/VersionedVerifierResolver.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {SymbioticVerifier} from "../../../src/chainlink/SymbioticVerifier.sol";
import {SettlementAlwaysValid} from "./SettlementAlwaysValid.sol";

/// @notice Shared scaffolding for the Chainlink CCV fork tests. Deploys a SymbioticVerifier
/// behind a fresh VersionedVerifierResolver, wires the remote-chain config, and registers the
/// resolver's inbound/outbound implementations. Concrete fork tests supply network-specific
/// addresses/selectors and layer their own test-specific setup on top.
abstract contract CCVForkBase is Test {
    bytes4 internal constant VERSION_TAG_V1_0_0 = 0x1a75bd93;

    string internal constant OPERATOR_LOCATION = "https://operator.example/verifications";

    SymbioticVerifier internal verifier;
    VersionedVerifierResolver internal resolver;

    /// @dev Deploys a SettlementAlwaysValid + SymbioticVerifier pair, wires the remote-chain
    /// config for `router`/`remoteChainSelector`, and registers the verifier as the resolver's
    /// inbound implementation for VERSION_TAG_V1_0_0.
    function _deployVerifierAndResolver(address rmnAddress, IRouter router, uint64 remoteChainSelector) internal {
        SettlementAlwaysValid settlement = new SettlementAlwaysValid();

        string[] memory locations = new string[](1);
        locations[0] = OPERATOR_LOCATION;
        verifier = new SymbioticVerifier(address(settlement), locations, rmnAddress, VERSION_TAG_V1_0_0);
        resolver = new VersionedVerifierResolver();

        BaseVerifier.RemoteChainConfigArgs[] memory args = new BaseVerifier.RemoteChainConfigArgs[](1);
        args[0] = BaseVerifier.RemoteChainConfigArgs({
            router: router,
            remoteChainSelector: remoteChainSelector,
            allowlistEnabled: false,
            feeUSDCents: 0,
            gasForVerification: 250_000,
            payloadSizeBytes: 1024
        });
        verifier.applyRemoteChainConfigUpdates(args);

        VersionedVerifierResolver.InboundImplementationArgs[] memory inbound =
            new VersionedVerifierResolver.InboundImplementationArgs[](1);
        inbound[0] = VersionedVerifierResolver.InboundImplementationArgs({
            version: VERSION_TAG_V1_0_0, verifier: address(verifier)
        });
        resolver.applyInboundImplementationUpdates(inbound);
    }

    /// @dev Registers `verifier` as the resolver's outbound implementation toward
    /// `destChainSelector`. Only needed by source-side fork tests that send messages.
    function _registerOutbound(uint64 destChainSelector) internal {
        VersionedVerifierResolver.OutboundImplementationArgs[] memory outbound =
            new VersionedVerifierResolver.OutboundImplementationArgs[](1);
        outbound[0] = VersionedVerifierResolver.OutboundImplementationArgs({
            destChainSelector: destChainSelector, verifier: address(verifier)
        });
        resolver.applyOutboundImplementationUpdates(outbound);
    }
}
