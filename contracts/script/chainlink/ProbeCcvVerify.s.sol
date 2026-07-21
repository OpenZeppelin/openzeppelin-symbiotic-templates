// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Script, console2} from "forge-std/Script.sol";

import {ISettlement as ISymbioticSettlement} from
    "@symbioticfi/relay-contracts/interfaces/modules/settlement/ISettlement.sol";
import {IKeyRegistry} from "@symbioticfi/relay-contracts/interfaces/modules/key-registry/IKeyRegistry.sol";
import {IVotingPowerProvider} from "@symbioticfi/relay-contracts/interfaces/modules/voting-power/IVotingPowerProvider.sol";

/// @notice Probe on-chain state at the failing CCV verify path.
/// Reads valSetHeader[0] params, committed validator-set hash, aggregated G1 pubkey,
/// and per-operator BLS key + voting power at capture timestamp.
///
/// Intent: surface whether the operators whose sidecars sign are actually present
/// in the committed validator set at epoch 0 with the same BLS keys.
///
/// Required env:
///   SETTLEMENT     — Sepolia Settlement address (default: testnet-ccv deployment)
///   KEY_REGISTRY   — Sepolia KeyRegistry address (default: testnet-ccv deployment)
///   VOTING_POWERS  — Sepolia VotingPowerProvider address (default: testnet-ccv deployment)
///
/// Run:
///   forge script script/ProbeCcvVerify.s.sol \
///     --rpc-url $DEST_RPC_URL --sig "run()" -vvv
contract ProbeCcvVerify is Script {
    uint32 internal constant VERIFICATION_TYPE = 1; // SigVerifierBlsBn254Simple
    uint8 internal constant KEY_TAG_BLS = 15;
    uint8 internal constant KEY_TAG_SECONDARY = 11;

    bytes32 internal constant KEY_TAG_PREFIX_HASH = keccak256("keyTag.");
    bytes32 internal constant VALIDATOR_SET_HASH_NAME = keccak256("validatorSetHashKeccak256");
    bytes32 internal constant AGG_PUB_KEY_G1_NAME = keccak256("aggPublicKeyG1");

    address internal constant DEFAULT_SETTLEMENT = 0x93fcf69144Cc0b8E270F934B045F5f6771e91B80;
    address internal constant DEFAULT_KEY_REGISTRY = 0x851eee8e30d785a6eA100fF5263A561721246a64;
    address internal constant DEFAULT_VOTING_POWERS = 0xCF8fAd7172475512992341F56f1267412eB32A78;

    function run() external view {
        address settlementAddr = vm.envOr("SETTLEMENT", DEFAULT_SETTLEMENT);
        address keyRegistryAddr = vm.envOr("KEY_REGISTRY", DEFAULT_KEY_REGISTRY);
        address votingPowersAddr = vm.envOr("VOTING_POWERS", DEFAULT_VOTING_POWERS);

        require(settlementAddr.code.length > 0, "Settlement: no code at address");
        require(keyRegistryAddr.code.length > 0, "KeyRegistry: no code at address");
        require(votingPowersAddr.code.length > 0, "VotingPowers: no code at address");

        ISymbioticSettlement settlement = ISymbioticSettlement(settlementAddr);
        IKeyRegistry keyRegistry = IKeyRegistry(keyRegistryAddr);
        IVotingPowerProvider votingPowers = IVotingPowerProvider(votingPowersAddr);

        console2.log("==== Settlement params ====");
        console2.log("settlement:        ", settlementAddr);
        uint48 lastEpoch = settlement.getLastCommittedHeaderEpoch();
        console2.log("lastCommittedEpoch:", uint256(lastEpoch));
        uint48 probeEpoch = uint48(vm.envOr("PROBE_EPOCH", uint256(0)));
        console2.log("probeEpoch:        ", uint256(probeEpoch));
        uint48 captureTs = settlement.getCaptureTimestampFromValSetHeaderAt(probeEpoch);
        require(captureTs != 0, "valSetHeader[probeEpoch] not committed");

        console2.log("captureTimestamp:  ", uint256(captureTs));
        console2.log("requiredKeyTag:    ", uint256(settlement.getRequiredKeyTagFromValSetHeaderAt(probeEpoch)));
        console2.log("quorumThreshold:   ", settlement.getQuorumThresholdFromValSetHeaderAt(probeEpoch));
        console2.log("totalVotingPower:  ", settlement.getTotalVotingPowerFromValSetHeaderAt(probeEpoch));
        console2.log("validatorsSszMRoot:");
        console2.logBytes32(settlement.getValidatorsSszMRootFromValSetHeaderAt(probeEpoch));

        // ---- committed validator set hash (per BlsBn254Simple) ----
        bytes32 valSetHashKey = _getKey(VERIFICATION_TYPE, KEY_TAG_BLS, VALIDATOR_SET_HASH_NAME);
        bytes32 committedValSetHash = settlement.getExtraDataAt(probeEpoch, valSetHashKey);
        console2.log("\n==== Committed BlsBn254Simple extra data ====");
        console2.log("valSetHashKey (slot):");
        console2.logBytes32(valSetHashKey);
        console2.log("committedValidatorSetHash:");
        console2.logBytes32(committedValSetHash);
        require(committedValSetHash != bytes32(0), "validatorSetHash extra data is zero");

        // ---- aggregated G1 pubkey (2 slots) ----
        bytes32 aggPubKeyG1SlotX = _getKey(VERIFICATION_TYPE, KEY_TAG_BLS, AGG_PUB_KEY_G1_NAME, 0);
        bytes32 aggPubKeyG1SlotY = _getKey(VERIFICATION_TYPE, KEY_TAG_BLS, AGG_PUB_KEY_G1_NAME, 1);
        bytes32 aggG1x = settlement.getExtraDataAt(probeEpoch, aggPubKeyG1SlotX);
        bytes32 aggG1y = settlement.getExtraDataAt(probeEpoch, aggPubKeyG1SlotY);
        console2.log("\naggregatedG1.X:");
        console2.logBytes32(aggG1x);
        console2.log("aggregatedG1.Y:");
        console2.logBytes32(aggG1y);

        // ---- operators in the active set at capture time ----
        address[] memory operators = votingPowers.getOperatorsAt(captureTs);
        console2.log("\n==== Operators at captureTimestamp ====");
        console2.log("count:", operators.length);
        for (uint256 i = 0; i < operators.length; i++) {
            address op = operators[i];
            console2.log("");
            console2.log("operator[", i, "]:", op);

            bytes memory key15 = keyRegistry.getKeyAt(op, KEY_TAG_BLS, captureTs);
            console2.log("  blsKey(tag=15) length:", key15.length);
            if (key15.length > 0) {
                bytes32 head;
                bytes32 tail;
                assembly {
                    head := mload(add(key15, 32))
                }
                console2.log("  blsKey(tag=15) first 32 bytes:");
                console2.logBytes32(head);
                if (key15.length >= 64) {
                    assembly {
                        tail := mload(add(key15, 64))
                    }
                    console2.log("  blsKey(tag=15) bytes 32..64:");
                    console2.logBytes32(tail);
                }
            }

            bytes memory key11 = keyRegistry.getKeyAt(op, KEY_TAG_SECONDARY, captureTs);
            console2.log("  blsKey(tag=11) length:", key11.length);

            // Voting power at captureTs (single-operator query, no extraData)
            try votingPowers.getOperatorVotingPowersAt(op, "", captureTs) returns (
                IVotingPowerProvider.VaultValue[] memory vaults
            ) {
                uint256 totalPower;
                for (uint256 j = 0; j < vaults.length; j++) {
                    totalPower += vaults[j].value;
                }
                console2.log("  votingPower(captureTs):", totalPower);
                console2.log("  vaultCount:", vaults.length);
            } catch {
                console2.log("  votingPower(captureTs): <call reverted>");
            }
        }

        // ---- live KeyRegistry comparison ----
        console2.log("\n==== Live KeyRegistry state (current block) ====");
        address[] memory liveOps = keyRegistry.getKeysOperatorsAt(captureTs);
        console2.log("getKeysOperatorsAt(captureTs):", liveOps.length);
        for (uint256 i = 0; i < liveOps.length; i++) {
            console2.log("  op:", liveOps[i]);
        }
    }

    // ─── helpers (mirror ExtraDataStorageHelper without importing the lib) ───

    function _getKey(uint32 verificationType, uint8 keyTag, bytes32 nameHash) internal pure returns (bytes32) {
        return keccak256(abi.encode(verificationType, KEY_TAG_PREFIX_HASH, keyTag, nameHash));
    }

    function _getKey(uint32 verificationType, uint8 keyTag, bytes32 nameHash, uint256 index)
        internal
        pure
        returns (bytes32)
    {
        return
            bytes32(uint256(keccak256(abi.encode(verificationType, KEY_TAG_PREFIX_HASH, keyTag, nameHash))) + index);
    }
}
