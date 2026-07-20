// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {Client} from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

/// @title MockRouter
/// @notice Minimal configurable CCIP router used by verifier tests and local deployments.
contract MockRouter is Ownable, IRouter {
    mapping(uint64 destChainSelector => address onRamp) private s_onRamps;
    mapping(uint64 sourceChainSelector => mapping(address offRamp => bool isConfigured)) private s_offRamps;

    constructor() Ownable(msg.sender) {}

    function setOnRamp(uint64 destChainSelector, address onRamp) external onlyOwner {
        s_onRamps[destChainSelector] = onRamp;
    }

    function setOffRamp(uint64 sourceChainSelector, address offRamp, bool isConfigured) external onlyOwner {
        s_offRamps[sourceChainSelector][offRamp] = isConfigured;
    }

    function getOnRamp(uint64 destChainSelector) external view override returns (address) {
        return s_onRamps[destChainSelector];
    }

    function isOffRamp(uint64 sourceChainSelector, address offRamp) external view override returns (bool) {
        return s_offRamps[sourceChainSelector][offRamp];
    }

    function routeMessage(
        Client.Any2EVMMessage calldata,
        uint16,
        uint256,
        address
    ) external pure override returns (bool success, bytes memory retBytes, uint256 gasUsed) {
        return (true, "", 0);
    }
}
