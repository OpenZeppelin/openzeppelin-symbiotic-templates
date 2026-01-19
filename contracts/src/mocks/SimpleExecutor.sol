// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {ILayerZeroExecutor} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/interfaces/ILayerZeroExecutor.sol";

/// @title SimpleExecutor
/// @notice Minimal executor implementing ILayerZeroExecutor for testing
/// @dev This executor accepts all jobs and returns a fixed fee.
///      It stores assigned jobs for inspection in tests.
///      DELETE THIS FILE before deploying to production.
contract SimpleExecutor is ILayerZeroExecutor {
    /// @notice Fixed fee returned for all jobs
    uint256 public nativeFee = 0.001 ether;

    /// @notice Job data stored for each assigned job
    struct Job {
        uint32 dstEid;
        address sender;
        uint256 calldataSize;
        bytes options;
    }

    /// @notice All jobs assigned to this executor
    Job[] public jobs;

    /// @notice Emitted when a job is assigned
    event JobAssigned(uint32 indexed dstEid, address indexed sender, uint256 calldataSize);

    /// @notice Set the native fee returned by this executor
    /// @param _fee The new fee amount
    function setNativeFee(uint256 _fee) external {
        nativeFee = _fee;
    }

    /// @notice Assigns a job to this executor
    /// @param _dstEid Destination endpoint ID
    /// @param _sender Source sending contract address
    /// @param _calldataSize Size of the message calldata
    /// @param _options Optional parameters for execution
    /// @return price The fee charged for this job
    function assignJob(
        uint32 _dstEid,
        address _sender,
        uint256 _calldataSize,
        bytes calldata _options
    ) external returns (uint256 price) {
        jobs.push(Job({dstEid: _dstEid, sender: _sender, calldataSize: _calldataSize, options: _options}));

        emit JobAssigned(_dstEid, _sender, _calldataSize);

        return nativeFee;
    }

    /// @notice Returns the fee for executing a message
    /// @return price The fee amount
    function getFee(uint32, address, uint256, bytes calldata) external view returns (uint256 price) {
        return nativeFee;
    }

    /// @notice Returns the number of jobs assigned
    /// @return count The job count
    function jobCount() external view returns (uint256 count) {
        return jobs.length;
    }

    /// @notice Returns a specific job by index
    /// @param _index The job index
    /// @return dstEid Destination endpoint ID
    /// @return sender Source sender address
    /// @return calldataSize Size of calldata
    /// @return options Execution options
    function getJob(uint256 _index)
        external
        view
        returns (uint32 dstEid, address sender, uint256 calldataSize, bytes memory options)
    {
        Job storage job = jobs[_index];
        return (job.dstEid, job.sender, job.calldataSize, job.options);
    }
}
