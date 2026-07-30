// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Test, Vm} from "forge-std/Test.sol";
import {stdJson} from "forge-std/StdJson.sol";

import {SymbioticLayerZeroDVN} from "../../src/layerzero/SymbioticLayerZeroDVN.sol";
import {ISettlement} from "../../src/interfaces/ISettlement.sol";
import {IReceiveUlnE2} from "../../src/layerzero/interfaces/IReceiveUlnE2.sol";
import {MockSendUln} from "../../src/layerzero/mocks/MockSendUln.sol";

contract SettlementStub is ISettlement {
    mapping(uint48 => uint48) public captureTimestampAt;

    function setCaptureTimestamp(uint48 epoch, uint48 ts) external {
        captureTimestampAt[epoch] = ts;
    }

    function verifyQuorumSigAt(
        bytes memory,
        uint8,
        uint256,
        bytes calldata,
        uint48,
        bytes memory
    ) external pure override returns (bool) {
        return true;
    }

    function getRequiredKeyTagFromValSetHeaderAt(uint48) external pure override returns (uint8) {
        return 0;
    }

    function getQuorumThresholdFromValSetHeaderAt(uint48) external pure override returns (uint256) {
        return 0;
    }

    function getCaptureTimestampFromValSetHeaderAt(uint48 epoch) external view override returns (uint48) {
        return captureTimestampAt[epoch];
    }
}

contract ReceiveUlnStub is IReceiveUlnE2 {
    function verify(bytes calldata, bytes32, uint64) external override {}

    function commitVerification(bytes calldata, bytes32) external override {}
}

contract IntegrationTest is Test {
    using stdJson for string;

    uint32 internal constant SOURCE_EID = 31337;
    uint32 internal constant DEST_EID = 31338;
    uint64 internal constant CONFIRMATIONS = 1;

    address internal constant SENDER = address(0xBEEF);
    address internal constant RECEIVER = address(0xCAFE);

    uint48 internal constant EPOCH = 1;

    bytes32 internal constant JOB_ASSIGNED_TOPIC =
        keccak256("JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)");

    // Destination chain contracts
    SymbioticLayerZeroDVN internal dvn;
    SettlementStub internal settlement;
    ReceiveUlnStub internal receiveUln;

    // Source chain contracts
    MockSendUln internal sourceSendUln;
    SymbioticLayerZeroDVN internal sourceDvn;

    address internal submitter;

    uint256 internal operatorPid;
    uint256 internal operatorPort;
    string internal operatorBaseUrl;
    string internal webhookSecret;

    function setUp() public {
        settlement = new SettlementStub();
        settlement.setCaptureTimestamp(EPOCH, uint48(block.timestamp));

        // Deploy destination chain contracts
        receiveUln = new ReceiveUlnStub();
        dvn = new SymbioticLayerZeroDVN(address(settlement), address(0), address(receiveUln), DEST_EID, 0);
        submitter = makeAddr("submitter");
        dvn.addSubmitter(submitter);

        // Deploy source chain contracts
        sourceSendUln = new MockSendUln(SOURCE_EID);
        sourceDvn = new SymbioticLayerZeroDVN(
            address(0), // no settlement on source
            address(sourceSendUln),
            address(0), // no receiveUln on source
            SOURCE_EID,
            0 // baseFee
        );
        sourceSendUln.setDvn(address(sourceDvn));

        webhookSecret = _defaultWebhookSecret();
    }

    function tearDown() public {
        if (operatorPid != 0) {
            _stopOperator(operatorPid);
        }
    }

    function test_operator_e2e_proof_submission() public {
        _startOperator();

        bytes memory packetHeader1 = _buildPacketHeader(1, 1, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash1 = keccak256("hello-1");
        bytes32 guid1 = keccak256(packetHeader1);

        bytes memory packetHeader2 = _buildPacketHeader(1, 2, SOURCE_EID, SENDER, DEST_EID, RECEIVER);
        bytes32 payloadHash2 = keccak256("hello-2");
        bytes32 guid2 = keccak256(packetHeader2);

        _sendWebhook(guid1, _jobData(packetHeader1, payloadHash1, 1));
        _sendWebhook(guid2, _jobData(packetHeader2, payloadHash2, 2));

        ProofData memory proof = _waitProof(guid1);
        require(proof.rootProof.length > 0, "root_proof empty");

        bytes memory signature = abi.encodePacked(bytes6(uint48(EPOCH)), proof.rootProof);

        vm.prank(submitter);
        dvn.submitProof(packetHeader1, payloadHash1, CONFIRMATIONS, proof.siblings, proof.rootHash, signature);

        bytes32 leaf = dvn.computeLeaf(packetHeader1, payloadHash1, CONFIRMATIONS);
        assertTrue(dvn.isLeafVerified(leaf));
        assertTrue(dvn.isRootVerified(proof.rootHash));
    }

    function test_operator_e2e_with_source_chain_contracts() public {
        _startOperator();

        // Send messages through actual source chain contracts
        bytes32 receiver = bytes32(uint256(uint160(RECEIVER)));
        (Vm.Log memory log1, bytes32 guid1) = _sendSourceMessage(DEST_EID, receiver, "hello-1");
        (Vm.Log memory log2, bytes32 guid2) = _sendSourceMessage(DEST_EID, receiver, "hello-2");

        // Decode event data from logs (instead of manual reconstruction)
        JobAssignedData memory job1 = _decodeJobAssigned(log1);
        JobAssignedData memory job2 = _decodeJobAssigned(log2);

        // Forward captured events to operator via webhook
        _sendWebhookFromLog(log1);
        _sendWebhookFromLog(log2);

        // Wait for operator to produce proofs for both messages
        ProofData memory proof1 = _waitProof(guid1);
        ProofData memory proof2 = _waitProof(guid2);
        require(proof1.rootProof.length > 0, "root_proof1 empty");
        require(proof2.rootProof.length > 0, "root_proof2 empty");

        // Submit proof for message 1 using decoded event data
        bytes memory signature1 = abi.encodePacked(bytes6(uint48(EPOCH)), proof1.rootProof);

        vm.prank(submitter);
        dvn.submitProof(job1.packetHeader, job1.payloadHash, job1.confirmations, proof1.siblings, proof1.rootHash, signature1);

        bytes32 leaf1 = dvn.computeLeaf(job1.packetHeader, job1.payloadHash, job1.confirmations);
        assertEq(proof1.leaf, leaf1, "proof1.leaf mismatch");
        assertTrue(dvn.isLeafVerified(leaf1), "leaf1 not verified");
        assertTrue(dvn.isRootVerified(proof1.rootHash), "root1 not verified");

        // Submit proof for message 2 using decoded event data
        // Root is already cached, so signature can be empty or reused
        bytes memory signature2 = abi.encodePacked(bytes6(uint48(EPOCH)), proof2.rootProof);

        vm.prank(submitter);
        dvn.submitProof(job2.packetHeader, job2.payloadHash, job2.confirmations, proof2.siblings, proof2.rootHash, signature2);

        bytes32 leaf2 = dvn.computeLeaf(job2.packetHeader, job2.payloadHash, job2.confirmations);
        assertEq(proof2.leaf, leaf2, "proof2.leaf mismatch");
        assertTrue(dvn.isLeafVerified(leaf2), "leaf2 not verified");
    }

    struct ProofData {
        bytes32 rootHash;
        bytes32 leaf;
        bytes32[] siblings;
        bytes rootProof;
    }

    function _startOperator() internal {
        string memory root = vm.projectRoot();
        string memory tmpDir = string.concat(root, "/.tmp");
        vm.createDir(tmpDir, true);

        uint256 uniq = _uniqueId();
        operatorPort = 18080 + (uniq % 1000) + 1;
        operatorBaseUrl = string.concat("http://127.0.0.1:", vm.toString(operatorPort));

        string memory configPath = string.concat(tmpDir, "/operator_config.json");
        string memory logPath = string.concat(tmpDir, "/operator.log");
        string memory dbPath = string.concat(tmpDir, "/operator_", vm.toString(uniq), ".db");

        string memory configJson = _buildConfigJson(dbPath, address(dvn));
        vm.writeFile(configPath, configJson);

        string[] memory cmd = new string[](7);
        cmd[0] = "env";
        cmd[1] = string.concat("WEBHOOK_SECRET=", webhookSecret);
        cmd[2] = string.concat("OZ_RELAYER_WEBHOOK_SECRET=", webhookSecret);
        cmd[3] = "bash";
        cmd[4] = string.concat(root, "/script/utils/start_operator.sh");
        cmd[5] = configPath;
        cmd[6] = logPath;

        string memory pidStr = string(vm.ffi(cmd));
        operatorPid = vm.parseUint(_trim(pidStr));

        _waitForHealth();
    }

    function _stopOperator(uint256 pid) internal {
        string[] memory cmd = new string[](3);
        cmd[0] = "bash";
        cmd[1] = string.concat(vm.projectRoot(), "/script/utils/stop_operator.sh");
        cmd[2] = vm.toString(pid);
        vm.ffi(cmd);
    }

    function _waitForHealth() internal {
        string[] memory cmd = new string[](4);
        cmd[0] = "python3";
        cmd[1] = string.concat(vm.projectRoot(), "/script/utils/wait_health.py");
        cmd[2] = string.concat(operatorBaseUrl, "/healthz");
        cmd[3] = "50";
        vm.ffi(cmd);
    }

    function _sendWebhook(bytes32 guid, bytes memory data) internal {
        bytes32 topic0 = keccak256(
            "JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)"
        );
        string memory topicsCsv = string.concat(vm.toString(topic0), ",", vm.toString(guid));

        string[] memory cmd = new string[](11);
        cmd[0] = "python3";
        cmd[1] = string.concat(vm.projectRoot(), "/script/utils/send_webhook.py");
        cmd[2] = string.concat(operatorBaseUrl, "/webhook/events");
        cmd[3] = webhookSecret;
        cmd[4] = vm.toString(uint256(SOURCE_EID));
        cmd[5] = vm.toString(uint256(1));
        cmd[6] = vm.toString(bytes32(uint256(0x1234)));
        cmd[7] = vm.toString(uint256(0));
        cmd[8] = vm.toString(address(dvn));
        cmd[9] = vm.toString(data);
        cmd[10] = topicsCsv;

        vm.ffi(cmd);
    }

    function _jobData(bytes memory packetHeader, bytes32 payloadHash, uint64 nonce) internal pure returns (bytes memory) {
        return abi.encode(
            SOURCE_EID,
            DEST_EID,
            SENDER,
            bytes32(uint256(uint160(RECEIVER))),
            payloadHash,
            packetHeader,
            CONFIRMATIONS,
            nonce,
            bytes(""),
            uint256(0)
        );
    }

    function _waitProof(bytes32 guid) internal returns (ProofData memory) {
        string[] memory cmd = new string[](7);
        cmd[0] = "python3";
        cmd[1] = string.concat(vm.projectRoot(), "/script/utils/wait_proof.py");
        cmd[2] = string.concat(operatorBaseUrl, "/api/v1/layerzero/proof");
        cmd[3] = vm.toString(guid);
        cmd[4] = "80";
        cmd[5] = "200";
        cmd[6] = "true";

        string memory json = string(vm.ffi(cmd));
        require(bytes(json).length > 2, "proof missing");

        ProofData memory proof;
        proof.rootHash = json.readBytes32(".root_hash");
        proof.leaf = json.readBytes32(".leaf");
        proof.siblings = json.readBytes32Array(".siblings");
        uint256[] memory rootProofVals = json.readUintArray(".root_proof");
        proof.rootProof = _bytesFromUintArray(rootProofVals);

        return proof;
    }

    struct JobAssignedData {
        uint32 srcEid;
        uint32 dstEid;
        address sender;
        bytes32 receiver;
        bytes32 payloadHash;
        bytes packetHeader;
        uint64 confirmations;
        uint64 nonce;
        bytes options;
        uint256 fee;
    }

    function _decodeJobAssigned(Vm.Log memory log) internal pure returns (JobAssignedData memory) {
        (
            uint32 srcEid,
            uint32 dstEid,
            address sender,
            bytes32 receiver,
            bytes32 payloadHash,
            bytes memory packetHeader,
            uint64 confirmations,
            uint64 nonce,
            bytes memory options,
            uint256 fee
        ) = abi.decode(log.data, (uint32, uint32, address, bytes32, bytes32, bytes, uint64, uint64, bytes, uint256));

        return JobAssignedData({
            srcEid: srcEid,
            dstEid: dstEid,
            sender: sender,
            receiver: receiver,
            payloadHash: payloadHash,
            packetHeader: packetHeader,
            confirmations: confirmations,
            nonce: nonce,
            options: options,
            fee: fee
        });
    }

    function _sendSourceMessage(
        uint32 dstEid,
        bytes32 receiver,
        bytes memory message
    ) internal returns (Vm.Log memory jobLog, bytes32 guid) {
        vm.recordLogs();

        vm.prank(SENDER);
        guid = sourceSendUln.sendMessage(dstEid, receiver, message, "");

        Vm.Log[] memory logs = vm.getRecordedLogs();
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == JOB_ASSIGNED_TOPIC && logs[i].emitter == address(sourceDvn)) {
                require(logs[i].topics[1] == guid, "guid mismatch");
                return (logs[i], guid);
            }
        }
        revert("JobAssigned not found");
    }

    function _sendWebhookFromLog(Vm.Log memory log) internal {
        string memory topicsCsv = "";
        for (uint256 i = 0; i < log.topics.length; i++) {
            if (i > 0) {
                topicsCsv = string.concat(topicsCsv, ",");
            }
            topicsCsv = string.concat(topicsCsv, vm.toString(log.topics[i]));
        }

        string[] memory cmd = new string[](11);
        cmd[0] = "python3";
        cmd[1] = string.concat(vm.projectRoot(), "/script/utils/send_webhook.py");
        cmd[2] = string.concat(operatorBaseUrl, "/webhook/events");
        cmd[3] = webhookSecret;
        cmd[4] = vm.toString(uint256(SOURCE_EID));
        cmd[5] = vm.toString(uint256(1));
        cmd[6] = vm.toString(bytes32(uint256(0x1234)));
        cmd[7] = vm.toString(uint256(0));
        cmd[8] = vm.toString(log.emitter);
        cmd[9] = vm.toString(log.data);
        cmd[10] = topicsCsv;

        vm.ffi(cmd);
    }

    function _buildConfigJson(
        string memory dbPath,
        address dvnAddress
    ) internal view returns (string memory) {
        return string.concat(
            "{",
            "\"server\":{\"host\":\"127.0.0.1\",\"port\":",
            vm.toString(operatorPort),
            ",\"read_timeout\":\"5s\",\"write_timeout\":\"5s\",\"idle_timeout\":\"30s\",\"security\":{\"timestamp_window\":\"1h\"}},",
            "\"database\":{\"path\":\"",
            dbPath,
            "\"},",
            "\"logging\":{\"level\":\"info\",\"format\":\"json\"},",
            "\"symbiotic_relay\":{\"address\":\"http://127.0.0.1:50051\",\"use_mock\":true},",
            "\"destination_chains\":[31338],",
            "\"signer\":{\"event_poll_interval\":\"200ms\",\"sign_job_interval\":\"200ms\",\"sign_worker_count\":1,\"min_batch_size\":2},",
            "\"oz_relayer\":{\"base_url\":\"http://127.0.0.1:8080\",\"chain_relayers\":[]},",
            "\"provider\":\"layerzero\",",
            "\"layerzero\":{\"eid_to_chain_id\":{\"31337\":31337,\"31338\":31338},\"dvn_addresses\":{\"31338\":\"",
            vm.toString(dvnAddress),
            "\"}}",
            "}"
        );
    }

    function _buildPacketHeader(
        uint8 version,
        uint64 nonce,
        uint32 srcEid,
        address sender,
        uint32 dstEid,
        address receiver
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            version,
            nonce,
            srcEid,
            bytes32(uint256(uint160(sender))),
            dstEid,
            bytes32(uint256(uint160(receiver)))
        );
    }

    function _defaultWebhookSecret() internal pure returns (string memory) {
        return "a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]a]";
    }

    function _bytesFromUintArray(uint256[] memory values) internal pure returns (bytes memory) {
        bytes memory out = new bytes(values.length);
        for (uint256 i = 0; i < values.length; i++) {
            out[i] = bytes1(uint8(values[i]));
        }
        return out;
    }

    function _uniqueId() internal returns (uint256) {
        string[] memory cmd = new string[](3);
        cmd[0] = "python3";
        cmd[1] = "-c";
        cmd[2] = "import time; print(int(time.time()*1000))";
        string memory out = string(vm.ffi(cmd));
        return vm.parseUint(_trim(out));
    }

    function _trim(string memory input) internal pure returns (string memory) {
        bytes memory data = bytes(input);
        uint256 start = 0;
        uint256 end = data.length;
        while (start < end && (data[start] == 0x20 || data[start] == 0x0a || data[start] == 0x0d || data[start] == 0x09)) {
            start++;
        }
        while (end > start && (data[end - 1] == 0x20 || data[end - 1] == 0x0a || data[end - 1] == 0x0d || data[end - 1] == 0x09)) {
            end--;
        }
        bytes memory out = new bytes(end - start);
        for (uint256 i = 0; i < end - start; i++) {
            out[i] = data[start + i];
        }
        return string(out);
    }
}
