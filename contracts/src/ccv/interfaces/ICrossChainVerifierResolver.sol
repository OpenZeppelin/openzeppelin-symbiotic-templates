// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

interface ICrossChainVerifierResolver {
    function getInboundImplementation(bytes calldata verifierResults) external view returns (address verifierAddress);

    function getOutboundImplementation(
        uint64 destChainSelector,
        bytes memory extraArgs
    ) external view returns (address verifierAddress);
}
