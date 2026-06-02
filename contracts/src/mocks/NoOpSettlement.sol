// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "../interfaces/ISettlement.sol";

/// @title NoOpSettlement
/// @notice Placeholder Settlement for the source side in dest-only Symbiotic deployments.
///
/// The source-chain `SymbioticCCV` only ever has its `forwardToVerifier` called by the
/// OnRamp; it never reads the Settlement. This contract exists so the constructor of
/// SymbioticCCV can accept a non-zero settlement address on chains where we deliberately
/// do not deploy the full Symbiotic relay infrastructure.
///
/// If anything ever does call this contract, every method reverts so the misconfiguration
/// is immediately visible rather than silently returning fake quorum-valid data.
contract NoOpSettlement is ISettlement {
    error NoOpSettlementShouldNotBeCalled();

    function verifyQuorumSigAt(bytes memory, uint8, uint256, bytes calldata, uint48, bytes memory)
        external
        pure
        override
        returns (bool)
    {
        revert NoOpSettlementShouldNotBeCalled();
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure override returns (uint8) {
        revert NoOpSettlementShouldNotBeCalled();
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure override returns (uint256) {
        revert NoOpSettlementShouldNotBeCalled();
    }

    function getCaptureTimestampFromValSetHeaderAt(uint48) external pure override returns (uint48) {
        revert NoOpSettlementShouldNotBeCalled();
    }
}
