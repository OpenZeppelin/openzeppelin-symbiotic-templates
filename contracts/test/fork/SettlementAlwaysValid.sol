// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ISettlement} from "../../src/interfaces/ISettlement.sol";

/// @notice Fork-test settlement stub: accepts any quorum signature so tests can
/// exercise the verifier path against real CCIP contracts without a live valset.
contract SettlementAlwaysValid is ISettlement {
    function verifyQuorumSigAt(bytes memory, uint8, uint256, bytes calldata, uint48, bytes memory)
        external
        pure
        override
        returns (bool)
    {
        return true;
    }
    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure override returns (uint8) {
        return 15;
    }
    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure override returns (uint256) {
        return 6600;
    }
    function getCaptureTimestampFromValSetHeaderAt(uint48) external view override returns (uint48) {
        return uint48(block.timestamp);
    }
}
