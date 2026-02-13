// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

library MessageV1Codec {
    struct MessageV1 {
        uint64 sourceChainSelector;
        uint64 destChainSelector;
        uint64 messageNumber;
        uint32 executionGasLimit;
        uint32 ccipReceiveGasLimit;
        uint16 finality;
        bytes32 ccvAndExecutorHash;
        bytes onRampAddress;
        bytes offRampAddress;
        bytes sender;
        bytes receiver;
        bytes destBlob;
        TokenTransferV1[] tokenTransfer;
        bytes data;
    }

    struct TokenTransferV1 {
        uint256 amount;
        bytes sourcePoolAddress;
        bytes sourceTokenAddress;
        bytes destTokenAddress;
        bytes tokenReceiver;
        bytes extraData;
    }
}
