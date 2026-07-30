// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/// @dev Subset of CCIP v2 IExecutor that the OnRamp calls during fee quoting.
interface IExecutor {
    function getAllowedFinalityConfig() external view returns (bytes4);
    function getFee(
        uint64 destChainSelector,
        bytes4 requestedFinalityConfig,
        address[] memory ccvAddresses,
        bytes memory extraArgs,
        address feeToken
    ) external view returns (uint16 usdCents);
}

/// @title NoOpExecutor
/// @notice Placeholder executor that quotes a zero fee and accepts all finality configs.
///
/// CCIP v2 requires the sender's extraArgs.executor to be a contract implementing
/// IExecutor. Setting it to an EOA causes the OnRamp's fee quote to revert.
/// This contract exists to be that address when the application owns its own
/// off-chain executor (i.e. our operator submits OffRamp.execute directly):
/// the OnRamp gets a valid IExecutor to query, the fee comes back zero, and
/// Chainlink's default ExecutorProxy is not used.
///
/// This contract has no execution role itself. The destination-side OffRamp.execute
/// is permissionless; whoever has valid verifierResults can call it.
contract NoOpExecutor is IExecutor {
    /// @dev Matches FinalityCodec.WAIT_FOR_FINALITY_FLAG.
    bytes4 internal constant WAIT_FOR_FINALITY_FLAG = 0x80000000;

    function getAllowedFinalityConfig() external pure override returns (bytes4) {
        return WAIT_FOR_FINALITY_FLAG;
    }

    function getFee(uint64, bytes4, address[] memory, bytes memory, address)
        external
        pure
        override
        returns (uint16)
    {
        return 0;
    }
}
