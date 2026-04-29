// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {SetDefaultUlnConfigParam} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/uln/UlnBase.sol";
import {SetDefaultExecutorConfigParam} from "@layerzerolabs/lz-evm-messagelib-v2/contracts/SendLibBase.sol";

/// @title Slim LayerZero V2 test harness
/// @notice Minimal stand-ins for EndpointV2Mock, SendUln302Mock, and ReceiveUln302Mock from
///         the upstream LayerZero test-devtools-evm-foundry package. Implements ONLY the
///         surface our local deploy script and integration test exercise: library registration,
///         default-library wiring, and ULN/executor config storage.
/// @dev    NOT a LayerZero protocol simulator. There is no real ULN logic, no message routing,
///         and no delivery semantics. The integration test asserts DVN state directly; these
///         contracts exist only so the surrounding plumbing compiles and the deploy script can
///         build a local devnet.

contract SlimEndpointV2 {
    uint32 public immutable eid;
    address public owner;

    mapping(address => bool) public isRegisteredLibrary;
    mapping(uint32 => address) public defaultSendLibrary;
    mapping(uint32 => address) public defaultReceiveLibrary;
    mapping(address => address) public delegates;

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
}

contract SlimSendUln302 {
    address public immutable endpoint;

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
        }
    }

    function setDefaultExecutorConfigs(SetDefaultExecutorConfigParam[] calldata params) external {
        delete _executorConfigs;
        for (uint256 i; i < params.length; i++) {
            _executorConfigs.push(params[i]);
        }
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
