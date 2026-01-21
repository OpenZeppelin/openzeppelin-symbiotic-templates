// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IReceiveUlnE2} from "../../src/interfaces/IReceiveUlnE2.sol";

contract RevertingReceiveUln is IReceiveUlnE2 {
    bool public shouldRevert = true;
    string public revertMessage = "ReceiveUln verification failed";

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function setRevertMessage(string calldata message) external {
        revertMessage = message;
    }

    function verify(bytes calldata, bytes32, uint64) external view override {
        if (shouldRevert) {
            revert(revertMessage);
        }
    }

    function commitVerification(bytes calldata, bytes32) external override {}
}
