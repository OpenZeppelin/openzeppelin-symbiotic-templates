// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, console2} from "forge-std/Test.sol";

import {IRouter} from "@chainlink/contracts-ccip/contracts/interfaces/IRouter.sol";
import {MessageV1Codec} from "@chainlink/contracts-ccip/contracts/libraries/MessageV1Codec.sol";
import {BaseVerifier} from "@chainlink/contracts-ccip/contracts/ccvs/components/BaseVerifier.sol";

import {BN254} from "@symbioticfi/relay-contracts/libraries/utils/BN254.sol";
import {SigVerifierBlsBn254Simple} from
    "@symbioticfi/relay-contracts/modules/settlement/sig-verifiers/SigVerifierBlsBn254Simple.sol";
import {ExtraDataStorageHelper} from
    "@symbioticfi/relay-contracts/modules/settlement/sig-verifiers/libraries/ExtraDataStorageHelper.sol";

import {SymbioticVerifier} from "../../src/chainlink/SymbioticVerifier.sol";
import {ISettlement} from "../../src/interfaces/ISettlement.sol";
import {MockRMN} from "../../src/chainlink/mocks/MockRMN.sol";
import {MockRouter} from "../../src/chainlink/mocks/MockRouter.sol";

/// @notice Settlement stand-in for the end-to-end benchmark: serves the extra-data slots the sig
/// verifier reads and forwards `verifyQuorumSigAt` to the real `SigVerifierBlsBn254Simple`,
/// mirroring the proof hand-off the real Settlement performs.
contract ForwardingSettlement is ISettlement {
    SigVerifierBlsBn254Simple internal immutable sigVerifier;

    mapping(bytes32 => bytes32) internal extraData;
    uint256 internal totalVotingPower;
    uint256 internal quorumThreshold;
    uint48 internal captureTimestamp;

    constructor(SigVerifierBlsBn254Simple sigVerifier_) {
        sigVerifier = sigVerifier_;
    }

    function setExtraData(bytes32 key, bytes32 value) external {
        extraData[key] = value;
    }

    function setTotalVotingPower(uint256 value) external {
        totalVotingPower = value;
    }

    function setQuorumThreshold(uint256 value) external {
        quorumThreshold = value;
    }

    function setCaptureTimestamp(uint48 value) external {
        captureTimestamp = value;
    }

    function verifyQuorumSigAt(
        bytes memory message,
        uint8 keyTag,
        uint256 threshold,
        bytes calldata proof,
        uint48 epoch,
        bytes memory
    ) external view override returns (bool) {
        return sigVerifier.verifyQuorumSig(address(this), epoch, message, keyTag, threshold, proof);
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure override returns (uint8) {
        return 15;
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external view override returns (uint256) {
        return quorumThreshold;
    }

    function getCaptureTimestampFromValSetHeaderAt(uint48) external view override returns (uint48) {
        return captureTimestamp;
    }

    function getExtraDataAt(uint48, bytes32 key) external view returns (bytes32) {
        return extraData[key];
    }

    function getTotalVotingPowerFromValSetHeaderAt(uint48) external view returns (uint256) {
        return totalVotingPower;
    }
}

/// @notice Gas benchmark for the simple (non-ZK) BLS verification path used by `verifyMessage`.
///
/// Builds synthetic equal-power validator sets of size N with K non-signers and measures two
/// things: the isolated `SigVerifierBlsBn254Simple.verifyQuorumSig` cost, and the end-to-end
/// `SymbioticVerifier.verifyMessage` cost through a forwarding Settlement (which includes the
/// verifier's proof-copy loops, whose cost grows with proof size and therefore with N and K).
///
/// Signature validity does not affect the gas profile: the aggregate signature is garbage (but
/// valid curve points), so the pairing runs at full cost and returns false. Assertions guard
/// that the full path — valset hash check, non-signer loop, and pairing — actually executed.
///
/// Note the quorum check is by voting power, not head count: with unequal weights, more than
/// N/3 validators can abstain while quorum still passes. The K = N/3 column is the equal-power
/// worst case, not a universal bound.
///
/// Run: forge test --match-contract VerificationGas -vv
contract VerificationGasTest is Test {
    using BN254 for BN254.G1Point;
    using ExtraDataStorageHelper for uint32;

    // Same tag as production operator keys (type 0 = BLS BN254, id 15).
    uint8 internal constant KEY_TAG = 15;
    uint32 internal constant VERIFICATION_TYPE = 1;
    uint256 internal constant VOTING_POWER = 1e18;
    bytes4 internal constant VERSION_TAG = 0x1a75bd93;
    uint64 internal constant SOURCE_CHAIN = 31337;
    uint64 internal constant DEST_CHAIN = 31338;

    SigVerifierBlsBn254Simple internal sigVerifier;
    ForwardingSettlement internal settlement;
    MockRouter internal router;
    MockRMN internal rmn;
    SymbioticVerifier internal verifier;

    address internal offRamp = makeAddr("offRamp");

    function setUp() public {
        sigVerifier = new SigVerifierBlsBn254Simple();
        settlement = new ForwardingSettlement(sigVerifier);
        settlement.setCaptureTimestamp(uint48(block.timestamp));
        router = new MockRouter();
        rmn = new MockRMN();

        string[] memory locations = new string[](1);
        locations[0] = "https://operator.example/verifications";
        verifier = new SymbioticVerifier(
            address(settlement), locations, address(rmn), VERSION_TAG, 48 hours, 2 hours
        );

        router.setOffRamp(SOURCE_CHAIN, offRamp, true);

        BaseVerifier.RemoteChainConfigArgs[] memory updates = new BaseVerifier.RemoteChainConfigArgs[](1);
        updates[0] = BaseVerifier.RemoteChainConfigArgs({
            router: IRouter(address(router)),
            remoteChainSelector: SOURCE_CHAIN,
            allowlistEnabled: false,
            feeUSDCents: 42,
            gasForVerification: 400_000,
            payloadSizeBytes: 128
        });
        verifier.applyRemoteChainConfigUpdates(updates);
    }

    function test_verificationGasTable() public {
        // Warm up account accesses so every measurement below is a warm-state call,
        // matching a verifyMessage call inside an already-executing OffRamp transaction.
        _measureSigVerify(3, 0);
        _measureVerifyMessage(3, 0);

        uint256[5] memory sizes = [uint256(3), 10, 25, 50, 100];
        console2.log("N | sigVerify K=0 | sigVerify K=N/3 | verifyMessage K=0 | verifyMessage K=N/3");
        for (uint256 i = 0; i < sizes.length; ++i) {
            uint256 n = sizes[i];
            console2.log(n, _measureSigVerify(n, 0), _measureSigVerify(n, n / 3));
            console2.log(n, _measureVerifyMessage(n, 0), _measureVerifyMessage(n, n / 3));
        }
    }

    function test_verificationGas_scalesMonotonically() public {
        _measureSigVerify(3, 0);
        assertLt(_measureSigVerify(3, 0), _measureSigVerify(100, 0));
        assertLt(_measureSigVerify(100, 0), _measureSigVerify(100, 33));

        _measureVerifyMessage(3, 0);
        assertLt(_measureVerifyMessage(3, 0), _measureVerifyMessage(100, 0));
        assertLt(_measureVerifyMessage(100, 0), _measureVerifyMessage(100, 33));
    }

    /// @dev Measures one isolated verifyQuorumSig call for a synthetic valset of `n` validators
    /// with the first `k` of them marked as non-signers.
    function _measureSigVerify(uint256 n, uint256 k) internal returns (uint256 gasUsed) {
        (bytes memory proof, uint256 quorumThreshold) = _prepareValset(n, k);
        bytes memory message = abi.encode(keccak256("digest"));

        uint256 gasBefore = gasleft();
        bool ok = sigVerifier.verifyQuorumSig(address(settlement), 1, message, KEY_TAG, quorumThreshold, proof);
        gasUsed = gasBefore - gasleft();

        // Garbage signature must fail — and only at the pairing, not on an early return.
        // The pairing check alone costs >100k gas, so a cheap run means the valset hash or
        // quorum check short-circuited and the measurement is invalid.
        assertFalse(ok);
        assertGt(gasUsed, 100_000);
    }

    /// @dev Measures the full verifyMessage path: BaseVerifier checks, the verifier's
    /// proof-copy loop, the Settlement hop, and the signature verification.
    function _measureVerifyMessage(uint256 n, uint256 k) internal returns (uint256 gasUsed) {
        (bytes memory proof, uint256 quorumThreshold) = _prepareValset(n, k);
        settlement.setQuorumThreshold(quorumThreshold);

        MessageV1Codec.MessageV1 memory message;
        message.sourceChainSelector = SOURCE_CHAIN;
        bytes memory verifierResults = abi.encodePacked(VERSION_TAG, bytes6(uint48(1)), proof);

        vm.prank(offRamp);
        uint256 gasBefore = gasleft();
        (bool success, bytes memory returndata) = address(verifier).call(
            abi.encodeCall(verifier.verifyMessage, (message, keccak256("message"), verifierResults))
        );
        gasUsed = gasBefore - gasleft();

        // The garbage signature must make verifyMessage revert with InvalidQuorumSignature —
        // the revert that fires only after the Settlement/sig-verifier path ran. Any other
        // selector means an earlier revert and an invalid measurement: at large N the gas
        // floor alone cannot catch that, because the proof-copy loop exceeds it by itself.
        assertFalse(success);
        assertEq(returndata, abi.encodeWithSelector(SymbioticVerifier.InvalidQuorumSignature.selector));
        assertGt(gasUsed, 100_000);
    }

    /// @dev Builds a synthetic valset of `n` validators (keys g1 * (i+1), equal voting power),
    /// commits its extra data on the settlement stub, and returns the proof with the first `k`
    /// validators as non-signers plus a 66% quorum threshold in voting-power units.
    function _prepareValset(uint256 n, uint256 k) internal returns (bytes memory proof, uint256 quorumThreshold) {
        bytes memory validatorsData = abi.encodePacked(uint256(n));
        BN254.G1Point memory aggKey;
        for (uint256 i = 0; i < n; ++i) {
            BN254.G1Point memory key = BN254.generatorG1().scalar_mul(i + 1);
            aggKey = i == 0 ? key : aggKey.plus(key);
            validatorsData = abi.encodePacked(validatorsData, _serializeG1(key), VOTING_POWER);
        }

        settlement.setExtraData(
            VERIFICATION_TYPE.getKey(KEY_TAG, sigVerifier.VALIDATOR_SET_HASH_KECCAK256_HASH()),
            keccak256(validatorsData)
        );
        settlement.setExtraData(
            VERIFICATION_TYPE.getKey(KEY_TAG, sigVerifier.AGGREGATED_PUBLIC_KEY_G1_HASH()), _serializeG1(aggKey)
        );
        settlement.setTotalVotingPower(n * VOTING_POWER);

        bytes memory nonSigners;
        for (uint256 i = 0; i < k; ++i) {
            nonSigners = abi.encodePacked(nonSigners, uint16(i));
        }

        // Aggregate signature (G1) and aggregate public key (G2): valid curve points with
        // garbage values — the pairing executes at identical cost and returns false.
        BN254.G2Point memory g2 = BN254.generatorG2();
        proof = abi.encodePacked(uint256(1), uint256(2), g2.X[0], g2.X[1], g2.Y[0], g2.Y[1], validatorsData, nonSigners);

        // 66% quorum by voting power: with k <= n/3 equal-power non-signers the threshold check
        // passes and the pairing is reached.
        quorumThreshold = (n * VOTING_POWER * 66) / 100;
    }

    function _serializeG1(BN254.G1Point memory p) internal view returns (bytes32) {
        (, uint256 derivedY) = BN254.findYFromX(p.X);
        return bytes32((p.X << 1) | (derivedY == p.Y ? 0 : 1));
    }
}
