// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IReceiveUlnE2} from "../../../src/layerzero/interfaces/IReceiveUlnE2.sol";
import {SymbioticLayerZeroDVN} from "../../../src/layerzero/SymbioticLayerZeroDVN.sol";

contract ReentrantReceiveUln is IReceiveUlnE2 {
    SymbioticLayerZeroDVN public dvn;

    // Second leaf parameters (different from first)
    bytes public reentryPacketHeader;
    bytes32 public reentryPayloadHash;
    uint64 public reentryConfirmations;
    bytes32[] public reentryProof;
    bytes32 public reentryMerkleRoot;
    bytes public reentrySignature;

    // Results
    bool public attempted;
    bool public reentrySucceeded;
    bytes public reentryRevertData;

    function setDvn(address dvnAddress) external {
        dvn = SymbioticLayerZeroDVN(payable(dvnAddress));
    }

    function configureReentry(
        bytes calldata packetHeader_,
        bytes32 payloadHash_,
        uint64 confirmations_,
        bytes32[] calldata proof_,
        bytes32 merkleRoot_,
        bytes calldata signature_
    ) external {
        reentryPacketHeader = packetHeader_;
        reentryPayloadHash = payloadHash_;
        reentryConfirmations = confirmations_;
        reentryProof = proof_;
        reentryMerkleRoot = merkleRoot_;
        reentrySignature = signature_;
    }

    function verify(bytes calldata, bytes32, uint64) external override {
        attempted = true;
        try dvn.submitProof(
            reentryPacketHeader,
            reentryPayloadHash,
            reentryConfirmations,
            reentryProof,
            reentryMerkleRoot,
            reentrySignature
        ) {
            reentrySucceeded = true;
        } catch (bytes memory err) {
            reentrySucceeded = false;
            reentryRevertData = err;
        }
    }

    function commitVerification(bytes calldata, bytes32) external override {}
}
