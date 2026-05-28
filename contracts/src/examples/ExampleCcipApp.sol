// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {IERC165} from "@openzeppelin/contracts/utils/introspection/IERC165.sol";

import {Client} from "../ccv/libraries/Client.sol";

/// @dev Subset of the real CCIP Router we need to call.
interface IRouterClient {
    function getFee(uint64 destinationChainSelector, Client.EVM2AnyMessage memory message)
        external
        view
        returns (uint256 fee);

    function ccipSend(uint64 destinationChainSelector, Client.EVM2AnyMessage calldata message)
        external
        payable
        returns (bytes32 messageId);
}

/// @dev Subset of `Any2EVMMessage` delivered by OffRamp via Router.routeMessage().
struct Any2EVMMessage {
    bytes32 messageId;
    uint64 sourceChainSelector;
    bytes sender;
    bytes data;
    Client.EVMTokenAmount[] destTokenAmounts;
}

interface IAny2EVMMessageReceiver {
    function ccipReceive(Any2EVMMessage calldata message) external;
}

interface IAny2EVMMessageReceiverV2 is IAny2EVMMessageReceiver {
    function getCCVsAndFinalityConfig(uint64 sourceChainSelector, bytes calldata sender)
        external
        view
        returns (
            address[] memory requiredCCVs,
            address[] memory optionalCCVs,
            uint8 optionalThreshold,
            bytes4 allowedFinalityConfig
        );
}

/// @title ExampleCcipApp
/// @notice Starter CCIP app demonstrating Symbiotic-secured CCV message verification.
///
/// One contract, deployed on both source and destination chains. Mirrors the
/// `ExampleOApp` pattern used for the LayerZero DVN template.
///
/// Message flow:
/// 1. User calls send() on source chain.
/// 2. Source ExampleCcipApp calls Router.ccipSend with extraArgs.ccvs = [SymbioticCCV]
///    and extraArgs.executor = configured operator (so Chainlink's default executor
///    is not used).
/// 3. Real CCIP OnRamp emits CCIPMessageSent.
/// 4. OZ Monitor + operator pick up the event, dispatch BLS signing through the
///    Symbiotic relay, and submit OffRamp.execute(...) on the destination chain.
/// 5. Destination OffRamp queries this contract's getCCVsAndFinalityConfig(),
///    which returns [SymbioticCCV_dest] as the required CCV.
/// 6. OffRamp verifies the BLS quorum signature via SymbioticCCV.verifyMessage.
/// 7. OffRamp invokes ccipReceive() on this contract via the Router.
contract ExampleCcipApp is Ownable, IAny2EVMMessageReceiverV2 {
    error OnlyRouter();
    error ZeroAddressNotAllowed();
    error UnknownRemoteApp(uint64 sourceChainSelector);
    error InvalidSenderEncoding();
    error UntrustedSender(uint64 sourceChainSelector, address sender);
    error InsufficientFee(uint256 quoted, uint256 provided);
    error NoRefundAvailable();
    error RefundWithdrawalFailed();

    event MessageSent(uint64 indexed destChainSelector, bytes32 indexed messageId, string message);
    event MessageReceived(uint64 indexed sourceChainSelector, bytes32 indexed messageId, address sender, string message);
    event RemoteAppSet(uint64 indexed remoteChainSelector, address remoteApp);
    event RefundCredited(address indexed account, uint256 amount);
    event RefundWithdrawn(address indexed account, uint256 amount);

    IRouterClient public immutable router;

    /// @notice Local SymbioticCCV deployment. Used as required CCV on receive
    /// and as the source-side CCV when sending.
    address public immutable ccv;

    /// @notice Operator address that will be paid the executor fee and is
    /// expected to call OffRamp.execute on the destination chain.
    address public immutable executor;

    /// @notice Trusted remote app addresses keyed by source chain selector.
    mapping(uint64 remoteChainSelector => address remoteApp) public remoteApp;
    mapping(address account => uint256 amount) public refundableBalance;

    bytes4 internal constant GENERIC_EXTRA_ARGS_V3_TAG = 0xa69dd4aa;
    bytes4 internal constant WAIT_FOR_FINALITY_FLAG = 0x80000000;

    constructor(address router_, address ccv_, address executor_) Ownable(msg.sender) {
        if (router_ == address(0) || ccv_ == address(0) || executor_ == address(0)) {
            revert ZeroAddressNotAllowed();
        }
        router = IRouterClient(router_);
        ccv = ccv_;
        executor = executor_;
    }

    /// @notice Register a trusted ExampleCcipApp on a remote chain.
    function setRemoteApp(uint64 remoteChainSelector, address remoteApp_) external onlyOwner {
        remoteApp[remoteChainSelector] = remoteApp_;
        emit RemoteAppSet(remoteChainSelector, remoteApp_);
    }

    /// @notice Send a string message to a remote chain.
    /// @param destChainSelector Destination chain selector (CCIP).
    /// @param message Arbitrary string payload.
    /// @param ccipReceiveGasLimit Gas limit for the destination ccipReceive callback.
    function send(uint64 destChainSelector, string calldata message, uint32 ccipReceiveGasLimit)
        external
        payable
        returns (bytes32 messageId)
    {
        address remote = remoteApp[destChainSelector];
        if (remote == address(0)) revert UnknownRemoteApp(destChainSelector);

        Client.EVM2AnyMessage memory msg_ = Client.EVM2AnyMessage({
            receiver: abi.encode(remote),
            data: abi.encode(message),
            tokenAmounts: new Client.EVMTokenAmount[](0),
            feeToken: address(0),
            extraArgs: _encodeExtraArgs(ccipReceiveGasLimit)
        });

        uint256 fee = router.getFee(destChainSelector, msg_);
        if (msg.value < fee) revert InsufficientFee(fee, msg.value);

        messageId = router.ccipSend{value: fee}(destChainSelector, msg_);

        if (msg.value > fee) {
            uint256 refund = msg.value - fee;
            (bool ok,) = msg.sender.call{value: refund}("");
            if (!ok) {
                refundableBalance[msg.sender] += refund;
                emit RefundCredited(msg.sender, refund);
            }
        }

        emit MessageSent(destChainSelector, messageId, message);
    }

    /// @notice Withdraw a previously credited refund when direct ETH refund failed.
    function withdrawRefund() external {
        uint256 amount = refundableBalance[msg.sender];
        if (amount == 0) revert NoRefundAvailable();

        refundableBalance[msg.sender] = 0;
        (bool ok,) = msg.sender.call{value: amount}("");
        if (!ok) {
            refundableBalance[msg.sender] = amount;
            revert RefundWithdrawalFailed();
        }

        emit RefundWithdrawn(msg.sender, amount);
    }

    /// @notice Quote the native fee to send a message.
    function quote(uint64 destChainSelector, string calldata message, uint32 ccipReceiveGasLimit)
        external
        view
        returns (uint256 fee)
    {
        address remote = remoteApp[destChainSelector];
        if (remote == address(0)) revert UnknownRemoteApp(destChainSelector);

        Client.EVM2AnyMessage memory msg_ = Client.EVM2AnyMessage({
            receiver: abi.encode(remote),
            data: abi.encode(message),
            tokenAmounts: new Client.EVMTokenAmount[](0),
            feeToken: address(0),
            extraArgs: _encodeExtraArgs(ccipReceiveGasLimit)
        });

        return router.getFee(destChainSelector, msg_);
    }

    /// @inheritdoc IAny2EVMMessageReceiverV2
    /// @dev Returns [ccv] as required, no optional CCVs, finality-required.
    function getCCVsAndFinalityConfig(uint64 /*sourceChainSelector*/, bytes calldata /*sender*/)
        external
        view
        override
        returns (
            address[] memory requiredCCVs,
            address[] memory optionalCCVs,
            uint8 optionalThreshold,
            bytes4 allowedFinalityConfig
        )
    {
        requiredCCVs = new address[](1);
        requiredCCVs[0] = ccv;
        optionalCCVs = new address[](0);
        optionalThreshold = 0;
        allowedFinalityConfig = WAIT_FOR_FINALITY_FLAG;
    }

    /// @inheritdoc IAny2EVMMessageReceiver
    function ccipReceive(Any2EVMMessage calldata m) external override {
        if (msg.sender != address(router)) revert OnlyRouter();
        if (m.sender.length != 32) revert InvalidSenderEncoding();

        address senderAddr = address(uint160(uint256(bytes32(m.sender))));
        address trusted = remoteApp[m.sourceChainSelector];
        if (trusted == address(0) || trusted != senderAddr) {
            revert UntrustedSender(m.sourceChainSelector, senderAddr);
        }

        string memory message = abi.decode(m.data, (string));
        emit MessageReceived(m.sourceChainSelector, m.messageId, senderAddr, message);
    }

    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IAny2EVMMessageReceiver).interfaceId
            || interfaceId == type(IAny2EVMMessageReceiverV2).interfaceId
            || interfaceId == type(IERC165).interfaceId;
    }

    /// @dev Encode GenericExtraArgsV3 with: our CCV (single), our executor (no args),
    /// no token transfer, requested finality = 0 (default wait-for-finality).
    /// Layout:
    ///   tag(4) | gasLimit(4) | requestedFinalityConfig(4) | ccvsLength(1) |
    ///   ccvAddrLength(1) | ccvAddr(20) | ccvArgsLength(2) |
    ///   executorLength(1) | executor(20) | executorArgsLength(2) |
    ///   tokenReceiverLength(1) | tokenArgsLength(2)
    function _encodeExtraArgs(uint32 gasLimit) internal view returns (bytes memory) {
        return abi.encodePacked(
            GENERIC_EXTRA_ARGS_V3_TAG,
            gasLimit,
            bytes4(0),
            uint8(1),
            uint8(20),
            bytes20(ccv),
            uint16(0),
            uint8(20),
            bytes20(executor),
            uint16(0),
            uint8(0),
            uint16(0)
        );
    }

    receive() external payable {}
}
