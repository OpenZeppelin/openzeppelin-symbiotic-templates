// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title SimpleExecutor
/// @notice Minimal executor for devnet testing
/// @dev Returns fixed fee and accepts all executions
contract SimpleExecutor {
    address public immutable owner;
    uint256 public nativeFee = 0.001 ether;

    constructor() {
        owner = msg.sender;
    }

    /// @notice Get the fee for executing on destination chain
    /// @dev Called by SendUln302 during quote
    function getFee(
        uint32, /*_dstEid*/
        address, /*_sender*/
        uint256, /*_calldataSize*/
        bytes calldata /*_options*/
    ) external view returns (uint256) {
        return nativeFee;
    }

    /// @notice Assign a job to the executor (called by SendUln302)
    function assignJob(
        uint32, /*_dstEid*/
        address, /*_sender*/
        uint256, /*_calldataSize*/
        bytes calldata /*_options*/
    ) external payable returns (uint256) {
        return nativeFee;
    }

    /// @notice Set the native fee
    function setNativeFee(uint256 _fee) external {
        require(msg.sender == owner, "Only owner");
        nativeFee = _fee;
    }

    /// @notice Withdraw accumulated fees
    function withdraw() external {
        require(msg.sender == owner, "Only owner");
        payable(owner).transfer(address(this).balance);
    }

    receive() external payable {}
}
