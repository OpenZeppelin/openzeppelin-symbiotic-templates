// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {SetDefaultUlnConfigParam} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {SetDefaultExecutorConfigParam} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";
import {
    MessagingFee,
    MessagingParams,
    MessagingReceipt
} from "@layerzerolabs/lz-evm-protocol-v2/contracts/interfaces/ILayerZeroEndpointV2.sol";
import {ILayerZeroDVN} from "../interfaces/ILayerZeroDVN.sol";

// Slim LayerZero V2 test harness.
//
// Minimal stand-ins for EndpointV2Mock, SendUln302Mock, and ReceiveUln302Mock from the upstream
// LayerZero test-devtools-evm-foundry package. Implements only the surface our local deploy
// script, integration test, and the `xtask msg send` / `make e2e` flow exercise: library
// registration, default-library wiring, ULN/executor config storage, and a single-DVN send path
// that forwards to ILayerZeroDVN.assignJob.
//
// NOT a LayerZero protocol simulator. There is no executor payment, no lzToken handling, no fee
// refund, and no real ULN verification. The send path assumes the destination chain has exactly
// one required DVN (which our local devnet does) and that confirmations is 1.

contract SlimEndpointV2 {
    error NoSendLibrary(uint32 dstEid);

    event PacketSent(
        address indexed sender, uint32 indexed dstEid, bytes32 receiver, uint64 nonce, bytes32 guid, uint256 fee
    );

    uint32 public immutable eid;
    address public owner;

    mapping(address => bool) public isRegisteredLibrary;
    mapping(uint32 => address) public defaultSendLibrary;
    mapping(uint32 => address) public defaultReceiveLibrary;
    mapping(address => address) public delegates;
    mapping(address sender => mapping(uint32 dstEid => mapping(bytes32 receiver => uint64))) public outboundNonce;

    constructor(uint32 _eid, address _owner) {
        eid = _eid;
        owner = _owner;
    }

    function registerLibrary(address lib) external {
        isRegisteredLibrary[lib] = true;
    }

    function setDefaultSendLibrary(uint32 destEid, address lib) external {
        defaultSendLibrary[destEid] = lib;
    }

    function setDefaultReceiveLibrary(uint32 srcEid, address lib, uint256 /* gracePeriod */ ) external {
        defaultReceiveLibrary[srcEid] = lib;
    }

    /// @notice Required by OAppCore constructor. Stored only — slim harness does no auth checks.
    function setDelegate(address delegate) external {
        delegates[msg.sender] = delegate;
    }

    /// @notice Quote the native fee for a send. Forwards to the configured default send library,
    ///         which queries its single configured DVN.
    function quote(MessagingParams calldata params, address sender) external view returns (MessagingFee memory) {
        address sendLib = defaultSendLibrary[params.dstEid];
        if (sendLib == address(0)) revert NoSendLibrary(params.dstEid);
        uint256 nativeFee = SlimSendUln302(sendLib).slimQuote(params.dstEid, sender, params.options);
        return MessagingFee({nativeFee: nativeFee, lzTokenFee: 0});
    }

    /// @notice Send a message. Builds the packet header + guid, increments per-channel nonce, and
    ///         forwards to the configured default send library which calls ILayerZeroDVN.assignJob.
    /// @dev    msg.value is forwarded in full to the DVN. No refund accounting.
    function send(MessagingParams calldata params, address /* refundAddress */ )
        external
        payable
        returns (MessagingReceipt memory receipt)
    {
        address sendLib = defaultSendLibrary[params.dstEid];
        if (sendLib == address(0)) revert NoSendLibrary(params.dstEid);

        uint64 nonce = ++outboundNonce[msg.sender][params.dstEid][params.receiver];
        (bytes32 guid, bytes memory packetHeader, bytes32 payloadHash) = _buildPacket(params, nonce);

        uint256 fee = SlimSendUln302(sendLib).slimSendPacket{value: msg.value}(
            params.dstEid, packetHeader, payloadHash, msg.sender, params.options
        );

        emit PacketSent(msg.sender, params.dstEid, params.receiver, nonce, guid, fee);
        receipt = MessagingReceipt({guid: guid, nonce: nonce, fee: MessagingFee({nativeFee: fee, lzTokenFee: 0})});
    }

    function _buildPacket(
        MessagingParams calldata params,
        uint64 nonce
    )
        internal
        view
        returns (bytes32 guid, bytes memory packetHeader, bytes32 payloadHash)
    {
        bytes32 sender32 = bytes32(uint256(uint160(msg.sender)));
        guid = keccak256(abi.encodePacked(nonce, eid, sender32, params.dstEid, params.receiver));
        packetHeader = abi.encodePacked(uint8(1), nonce, eid, sender32, params.dstEid, params.receiver);
        payloadHash = keccak256(abi.encodePacked(guid, params.message));
    }
}

contract SlimSendUln302 {
    uint64 public constant SLIM_CONFIRMATIONS = 1;

    error DvnNotConfigured(uint32 dstEid);

    event PacketSent(uint32 indexed dstEid, bytes packetHeader, bytes32 payloadHash, uint256 fee);

    address public immutable endpoint;

    /// @notice DVN address per destination EID, populated from `setDefaultUlnConfigs`.
    mapping(uint32 dstEid => address) public requiredDvn;

    SetDefaultUlnConfigParam[] private _ulnConfigs;
    SetDefaultExecutorConfigParam[] private _executorConfigs;

    constructor(
        address payable, /* testHelper */
        address _endpoint,
        uint256, /* treasuryGasCap */
        uint256 /* treasuryGasForFeeCap */
    ) {
        endpoint = _endpoint;
    }

    function setDefaultUlnConfigs(SetDefaultUlnConfigParam[] calldata params) external {
        delete _ulnConfigs;
        for (uint256 i; i < params.length; i++) {
            _ulnConfigs.push(params[i]);
            if (params[i].config.requiredDVNs.length > 0) {
                requiredDvn[params[i].eid] = params[i].config.requiredDVNs[0];
            }
        }
    }

    function setDefaultExecutorConfigs(SetDefaultExecutorConfigParam[] calldata params) external {
        delete _executorConfigs;
        for (uint256 i; i < params.length; i++) {
            _executorConfigs.push(params[i]);
        }
    }

    /// @notice Quote fee for a send by querying the single configured DVN.
    function slimQuote(uint32 dstEid, address sender, bytes calldata options) external view returns (uint256) {
        address dvn = requiredDvn[dstEid];
        if (dvn == address(0)) revert DvnNotConfigured(dstEid);
        return ILayerZeroDVN(dvn).getFee(dstEid, SLIM_CONFIRMATIONS, sender, options);
    }

    /// @notice Forward the packet to the configured DVN's assignJob and return its fee.
    /// @dev    Accepts msg.value from the endpoint (the OApp's native fee) but does NOT forward
    ///         it to assignJob — the DVN explicitly rejects ETH (`NoFeeAccepted`). Real LZ would
    ///         pay the executor/treasury here; the slim harness just lets the value sit so the
    ///         OApp.send call doesn't revert.
    function slimSendPacket(
        uint32 dstEid,
        bytes calldata packetHeader,
        bytes32 payloadHash,
        address sender,
        bytes calldata options
    )
        external
        payable
        returns (uint256 fee)
    {
        address dvn = requiredDvn[dstEid];
        if (dvn == address(0)) revert DvnNotConfigured(dstEid);
        ILayerZeroDVN.AssignJobParam memory param = ILayerZeroDVN.AssignJobParam({
            dstEid: dstEid,
            packetHeader: packetHeader,
            payloadHash: payloadHash,
            confirmations: SLIM_CONFIRMATIONS,
            sender: sender
        });
        fee = ILayerZeroDVN(dvn).assignJob(param, options);
        emit PacketSent(dstEid, packetHeader, payloadHash, fee);
    }
}

contract SlimReceiveUln302 {
    address public immutable endpoint;

    SetDefaultUlnConfigParam[] private _ulnConfigs;

    constructor(address _endpoint) {
        endpoint = _endpoint;
    }

    function setDefaultUlnConfigs(SetDefaultUlnConfigParam[] calldata params) external {
        delete _ulnConfigs;
        for (uint256 i; i < params.length; i++) {
            _ulnConfigs.push(params[i]);
        }
    }

    /// @notice No-op: DVN calls this from `submitProof`. Tests assert DVN state directly,
    ///         not ULN-side verification accounting.
    function verify(bytes calldata, /* packetHeader */ bytes32, /* payloadHash */ uint64 /* confirmations */ )
        external
    { }

    /// @notice No-op: tests assert DVN state directly; full ULN commit logic is out of scope.
    function commitVerification(bytes calldata, /* packetHeader */ bytes32 /* payloadHash */ ) external { }
}
