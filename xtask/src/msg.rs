use std::fs;
use std::time::{Duration, Instant};

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256, Bytes, FixedBytes, Log as PrimitiveLog, U256, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::SolEvent;
use eyre::{Result, bail, eyre};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::cli::{MsgArgs, MsgCommand, MsgE2eArgs, MsgSendArgs, MsgWatchArgs};
use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig, Provider as ActiveProvider};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::genesis;
use crate::runtime::{self, RuntimeInputs};
use crate::ui;

const MESSAGE_READY_TIMEOUT_SECONDS: u64 = 180;
const MESSAGE_READY_POLL_SECONDS: u64 = 2;
const MESSAGE_READY_MAX_LAG_BLOCKS: u64 = 20;
const SOURCE_LOG_RETRY_ATTEMPTS: u64 = 15;
const SOURCE_LOG_RETRY_SECONDS: u64 = 2;
const WATCH_POLL_SECONDS: u64 = 2;
const MAX_LOG_BLOCK_RANGE: u64 = 10;
const OPERATOR_PORTS: [u16; 3] = [3001, 3002, 3003];
/// Mock-message executionGasLimit. Real OnRamps set it to the summed destination
/// gas of every component, including the CCV's verification reservation. BLS
/// quorum verification on the local devnet needs ~700k; over-sizing is harmless
/// (only gas used is billed), so reserve 1M for verification headroom.
const MOCK_EXECUTION_GAS_LIMIT: u32 = 1_000_000;
/// Mock-mode version tag (`VERSION_TAG_V1_0_0` from MessageV1Codec). Only used
/// for the local Anvil send path through `MockCCIPOnRamp.sendMessage`; real
/// CCIP encodes this inside the protocol's own message format.
const DEFAULT_CCV_VERSION_TAG: &str = "0x1a75bd93";
const CCV_MESSAGE_EXECUTED_EVENT: &str = "MessageExecuted(bytes32,uint256,uint256)";

sol! {
    struct MessagingFee {
        uint256 nativeFee;
        uint256 lzTokenFee;
    }

    #[sol(rpc)]
    interface ExampleOApp {
        event MessageSent(uint32 indexed dstEid, string message, bytes32 guid, uint64 nonce);
        function buildOptions(uint128 _gas) external pure returns (bytes memory options);
        function quote(uint32 _dstEid, string calldata _message, bytes calldata _options, bool _payInLzToken)
            external
            view
            returns (MessagingFee memory fee);
        function send(uint32 _dstEid, string calldata _message, bytes calldata _options) external payable;
    }

    #[sol(rpc)]
    interface ExampleCcipApp {
        event MessageSent(uint64 indexed destChainSelector, bytes32 indexed messageId, string message);
        function send(uint64 destChainSelector, string calldata message, uint32 ccipReceiveGasLimit)
            external
            payable
            returns (bytes32 messageId);
        function quote(uint64 destChainSelector, string calldata message, uint32 ccipReceiveGasLimit)
            external
            view
            returns (uint256 fee);
    }

    /// Mock OnRamp used only on local Anvil. Real CCIP has no equivalent ABI —
    /// senders go through `Router.ccipSend`, which isn't deployed on the local
    /// stack. Kept here so the local e2e path keeps working without spinning
    /// up a router.
    struct MockCcipReceipt {
        address issuer;
        uint32 destGasLimit;
        uint32 destBytesOverhead;
        uint256 feeTokenAmount;
        bytes extraArgs;
    }

    #[sol(rpc)]
    interface MockCCIPOnRamp {
        event CCIPMessageSent(
            uint64 indexed destChainSelector,
            address indexed sender,
            bytes32 indexed messageId,
            address feeToken,
            uint256 tokenAmountBeforeTokenPoolFees,
            bytes encodedMessage,
            MockCcipReceipt[] receipts,
            bytes[] verifierBlobs
        );
        function sendMessage(uint64 destChainSelector, bytes calldata encodedMessage, bytes4 versionTag, address executor)
            external
            returns (bytes32 messageId);
        function nonce() external view returns (uint64 value);
    }

    #[sol(rpc)]
    interface MockCCIPOffRamp {
        event MessageExecuted(bytes32 indexed messageId, uint256 ccvCount, uint256 verifierResultCount);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageCache {
    tx_hash: B256,
    block: u64,
    message_id: B256,
    message: String,
    /// The address designated as the message's executor at send time, if
    /// known. Threaded through to `watch` so a timeout can name the
    /// designated executor in its diagnostic hint. Absent in caches written
    /// before this field existed.
    #[serde(default)]
    executor: Option<Address>,
}

#[derive(Debug, Clone)]
struct LayerZeroMessageContext {
    source_rpc: String,
    dest_rpc: String,
    private_key: String,
    source_oapp: Address,
    destination_target: Address,
    dest_eid: u32,
}

#[derive(Debug, Clone)]
struct CcvMessageContext {
    source_rpc: String,
    dest_rpc: String,
    private_key: String,
    destination_offramp: Address,
    source_chain_selector: u64,
    dest_chain_selector: u64,
    send_mode: CcvSendMode,
}

/// Two distinct source-send paths. Real CCIP requires going through a
/// Router-bound app contract; local mocks don't have a Router and call the
/// OnRamp directly.
#[derive(Debug, Clone)]
enum CcvSendMode {
    /// Production / staging: send via `ExampleCcipApp.send` which calls
    /// `router.ccipSend` with the quoted native fee.
    RealCcip { source_example_app: Address },
    /// Local Anvil: call `MockCCIPOnRamp.sendMessage` directly with the
    /// version tag the destination mock expects.
    Mock {
        source_onramp: Address,
        version_tag: FixedBytes<4>,
    },
}

#[derive(Debug, Clone)]
enum MessageContext {
    LayerZero(LayerZeroMessageContext),
    ChainlinkCcv(CcvMessageContext),
}

#[derive(Debug, Clone)]
struct SentMessage {
    tx_hash: B256,
    block: u64,
    message_id: B256,
}

#[derive(Debug, Clone)]
struct SourceEventLog {
    tx_hash: Option<B256>,
    log: PrimitiveLog,
}

#[derive(Debug, Clone)]
struct WatchTarget {
    tx_hash: B256,
    message_id: B256,
    start_block: u64,
    /// The address designated as the message's executor at send time, if
    /// known. Used only to enrich the timeout error when the relayer reports
    /// the message as skipped.
    executor: Option<Address>,
}

#[derive(Debug, Clone, Serialize)]
struct WatchOutcome {
    status: &'static str,
    message_id: B256,
    dest_tx: B256,
    elapsed: u64,
}

#[derive(Debug, Deserialize)]
struct OperatorMessagesResponse {
    #[serde(default)]
    messages: Vec<OperatorMessage>,
}

#[derive(Debug, Deserialize)]
struct OperatorMessage {
    metadata: OperatorMetadata,
    status: String,
    #[serde(default)]
    submission: Option<OperatorSubmission>,
}

#[derive(Debug, Deserialize)]
struct OperatorMetadata {
    message_id: B256,
    event_tx_hash: B256,
}

#[derive(Debug, Deserialize)]
struct OperatorSubmission {
    state: String,
    #[serde(default)]
    tx_hash: Option<B256>,
    #[serde(default)]
    last_error: Option<String>,
    /// On-chain message-level outcome (Success | Failure). Populated by the
    /// operator when the destination OffRamp emits ExecutionStateChanged.
    /// Authoritative for "did the message deliver?" — independent of which
    /// operator's tx mined and of whether the outer tx succeeded.
    #[serde(default)]
    execution_state: Option<String>,
    /// Tx that drove the on-chain state change. May differ from `tx_hash` when
    /// a peer operator won the race.
    #[serde(default)]
    delivery_tx_hash: Option<B256>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct WatchProgress {
    operator_status: Option<String>,
    submission_state: Option<String>,
    submission_tx: Option<B256>,
    submission_error: Option<String>,
    execution_state: Option<String>,
    delivery_tx: Option<B256>,
}

impl MessageContext {
    fn provider_name(&self) -> &'static str {
        match self {
            Self::LayerZero(_) => "layerzero",
            Self::ChainlinkCcv(_) => "chainlink_ccv",
        }
    }

    fn source_rpc(&self) -> &str {
        match self {
            Self::LayerZero(context) => &context.source_rpc,
            Self::ChainlinkCcv(context) => &context.source_rpc,
        }
    }

    fn dest_rpc(&self) -> &str {
        match self {
            Self::LayerZero(context) => &context.dest_rpc,
            Self::ChainlinkCcv(context) => &context.dest_rpc,
        }
    }
}

pub fn run_command(context: &ResolvedContext, args: &MsgArgs) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = load_deployments_or_bail(context)?;

    if matches!(args.command, MsgCommand::E2e(_)) {
        preflight_check(context)?;
    }
    let runtime = RuntimeInputs::resolve(context, &env_config);
    let msg_context = load_message_context(context, &env_config, &deployments, &runtime)?;

    if !matches!(
        &args.command,
        MsgCommand::Watch(MsgWatchArgs { json: true, .. })
    ) {
        let command_name = match args.command {
            MsgCommand::Send(_) => "msg send",
            MsgCommand::Watch(_) => "msg watch",
            MsgCommand::E2e(_) => "msg e2e",
        };
        if !command_uses_json(&args.command) {
            ui::header(
                command_name,
                &context.env_name,
                Some(msg_context.provider_name()),
            );
        }
    }

    match &args.command {
        MsgCommand::Send(send) => run_send(context, &env_config, &msg_context, send),
        MsgCommand::Watch(watch) => run_watch_command(context, &env_config, &msg_context, watch),
        MsgCommand::E2e(e2e) => run_e2e(context, &env_config, &msg_context, e2e),
    }
}

fn command_uses_json(command: &MsgCommand) -> bool {
    match command {
        MsgCommand::Send(args) => args.json,
        MsgCommand::Watch(args) => args.json,
        MsgCommand::E2e(args) => args.json,
    }
}

fn run_send(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &MessageContext,
    args: &MsgSendArgs,
) -> Result<()> {
    let finality = parse_finality_flag(args.finality.as_deref())?;
    let executor = parse_executor_flag(args.executor.as_deref(), env_config)?;
    let sent = send_message(
        context,
        env_config,
        msg_context,
        &args.message,
        args.gas,
        finality,
        executor,
    )?;
    save_cache(
        context,
        &MessageCache {
            tx_hash: sent.tx_hash,
            block: sent.block,
            message_id: sent.message_id,
            message: args.message.clone(),
            executor: Some(executor),
        },
    )?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "provider": msg_context.provider_name(),
                "tx_hash": sent.tx_hash,
                "block": sent.block,
                "message_id": sent.message_id,
            }))?
        );
    } else {
        ui::ok("message submitted");
        ui::detail("message", &args.message);
        ui::detail("message_id", sent.message_id);
        ui::detail("tx", sent.tx_hash);
        ui::detail("block", sent.block);
        ui::next(&format!("make watch ENV={}", context.env_name));
    }

    Ok(())
}

fn run_watch_command(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &MessageContext,
    args: &MsgWatchArgs,
) -> Result<()> {
    wait_for_message_readiness(context, env_config, msg_context.source_rpc())?;
    let target = resolve_watch_target(context, msg_context.dest_rpc(), args)?;
    let outcome = watch_message(msg_context, target, args.timeout, args.json)?;

    if args.json {
        println!("{}", serde_json::to_string(&outcome)?);
    }

    Ok(())
}

fn run_e2e(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &MessageContext,
    args: &MsgE2eArgs,
) -> Result<()> {
    let finality = parse_finality_flag(args.finality.as_deref())?;
    let executor = parse_executor_flag(args.executor.as_deref(), env_config)?;
    let sent = send_message(
        context,
        env_config,
        msg_context,
        &args.message,
        args.gas,
        finality,
        executor,
    )?;
    save_cache(
        context,
        &MessageCache {
            tx_hash: sent.tx_hash,
            block: sent.block,
            message_id: sent.message_id,
            message: args.message.clone(),
            executor: Some(executor),
        },
    )?;

    wait_for_message_readiness(context, env_config, msg_context.source_rpc())?;
    let outcome = watch_message(
        msg_context,
        WatchTarget {
            tx_hash: sent.tx_hash,
            message_id: sent.message_id,
            start_block: sent.block,
            executor: Some(executor),
        },
        args.timeout,
        args.json,
    )?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "provider": msg_context.provider_name(),
                "tx_hash": sent.tx_hash,
                "block": sent.block,
                "message_id": sent.message_id,
                "watch": outcome,
            }))?
        );
    }

    Ok(())
}

fn load_message_context(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
) -> Result<MessageContext> {
    match env_config.active_provider {
        ActiveProvider::LayerZero => Ok(MessageContext::LayerZero(load_layerzero_context(
            env_config,
            deployments,
            runtime,
        )?)),
        ActiveProvider::ChainlinkCcv => Ok(MessageContext::ChainlinkCcv(load_ccv_context(
            context,
            env_config,
            deployments,
            runtime,
        )?)),
    }
}

fn load_layerzero_context(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
) -> Result<LayerZeroMessageContext> {
    let source_rpc = runtime
        .source_rpc
        .clone()
        .ok_or_else(|| eyre!("SOURCE RPC is not configured"))?;
    let dest_rpc = runtime
        .dest_rpc
        .clone()
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;
    let private_key = runtime
        .private_key
        .clone()
        .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?;
    let source_oapp = deployments
        .layerzero_oapp_deployment(ChainRole::Source)
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| missing_layerzero_oapp(env_config))?;
    let destination_target = deployments
        .deployment(ChainRole::Destination, "layerzero.dvn")
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| eyre!("missing destination DVN deployment"))?;

    Ok(LayerZeroMessageContext {
        source_rpc,
        dest_rpc,
        private_key,
        source_oapp,
        destination_target,
        dest_eid: env_config.chains.destination.eid,
    })
}

fn load_ccv_context(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
) -> Result<CcvMessageContext> {
    let source_rpc = runtime
        .source_rpc
        .clone()
        .ok_or_else(|| eyre!("SOURCE RPC is not configured"))?;
    let dest_rpc = runtime
        .dest_rpc
        .clone()
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;
    let private_key = runtime
        .private_key
        .clone()
        .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?;
    let destination_offramp = runtime::setting(context, "CCV_DEST_OFFRAMP_ADDRESS")
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp"))
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| eyre!("missing destination CCV offRamp deployment"))?;
    let dest_chain_selector =
        if let Some(selector) = runtime::setting(context, "CCV_DEST_CHAIN_SELECTOR") {
            selector.parse()?
        } else {
            env_config.ccip_selector(ChainRole::Destination)?
        };
    let source_chain_selector =
        if let Some(selector) = runtime::setting(context, "CCV_SOURCE_CHAIN_SELECTOR") {
            selector.parse()?
        } else {
            env_config.ccip_selector(ChainRole::Source)?
        };

    // Local Anvil has no CCIP Router, so the send path can't go through
    // ExampleCcipApp.send → router.ccipSend. Fall back to calling
    // MockCCIPOnRamp.sendMessage directly. Non-local environments use the
    // real-CCIP path that bills native fee through the Router.
    let send_mode = if env_config.is_local() {
        let source_onramp = runtime::setting(context, "CCV_SOURCE_ONRAMP_ADDRESS")
            .filter(|value| !value.is_empty())
            .or_else(|| deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp"))
            .and_then(|value| parse_address(&value))
            .ok_or_else(|| eyre!("missing source CCV onRamp deployment for local send path"))?;
        let version_tag = runtime::setting(context, "CCV_VERSION_TAG")
            .unwrap_or_else(|| DEFAULT_CCV_VERSION_TAG.to_string())
            .parse()?;
        CcvSendMode::Mock {
            source_onramp,
            version_tag,
        }
    } else {
        let source_example_app = runtime::setting(context, "CCV_SOURCE_EXAMPLE_APP_ADDRESS")
            .filter(|value| !value.is_empty())
            .or_else(|| deployments.deployment(ChainRole::Source, "chainlinkCcv.exampleApp"))
            .and_then(|value| parse_address(&value))
            .ok_or_else(|| {
                eyre!(
                    "missing source CCV ExampleCcipApp deployment; run `make deploy ENV={}`",
                    context.env_name
                )
            })?;
        CcvSendMode::RealCcip { source_example_app }
    };

    Ok(CcvMessageContext {
        source_rpc,
        dest_rpc,
        private_key,
        destination_offramp,
        source_chain_selector,
        dest_chain_selector,
        send_mode,
    })
}

fn send_message(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &MessageContext,
    message: &str,
    gas: u128,
    finality: u32,
    executor: Address,
) -> Result<SentMessage> {
    match msg_context {
        MessageContext::LayerZero(layerzero) => send_layerzero_message(layerzero, message, gas),
        MessageContext::ChainlinkCcv(ccv) => {
            maybe_refresh_ccv_epoch(context, env_config)?;
            send_ccv_message(ccv, message, gas, finality, executor)
        }
    }
}

fn send_layerzero_message(
    msg_context: &LayerZeroMessageContext,
    message: &str,
    gas: u128,
) -> Result<SentMessage> {
    let signer: PrivateKeySigner = msg_context.private_key.parse()?;
    let wallet = EthereumWallet::from(signer);
    let source_rpc = msg_context.source_rpc.clone();
    let source_oapp = msg_context.source_oapp;
    let dest_eid = msg_context.dest_eid;
    let message = message.to_string();

    block_on(async move {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(source_rpc.parse()?);
        let contract = ExampleOApp::new(source_oapp, provider.clone());

        let options: Bytes = contract.buildOptions(gas).call().await?.options;
        let quote = contract
            .quote(dest_eid, message.clone(), options.clone(), false)
            .call()
            .await?;
        let fee: U256 = quote.fee.nativeFee;

        let pending = contract
            .send(dest_eid, message.clone(), options)
            .value(fee)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        let tx_hash = receipt.transaction_hash;
        let block = receipt
            .block_number
            .ok_or_else(|| eyre!("transaction receipt missing block number"))?;

        let message_id = source_event_id_with_retry(
            tx_hash,
            SOURCE_LOG_RETRY_ATTEMPTS,
            Duration::from_secs(SOURCE_LOG_RETRY_SECONDS),
            || async {
                let logs = provider
                    .get_logs(
                        &Filter::new()
                            .address(source_oapp)
                            .from_block(block)
                            .to_block(block),
                    )
                    .await?;
                Ok(logs
                    .into_iter()
                    .map(|log| SourceEventLog {
                        tx_hash: log.transaction_hash,
                        log: PrimitiveLog {
                            address: log.inner.address,
                            data: log.inner.data.clone(),
                        },
                    })
                    .collect())
            },
            |log| {
                ExampleOApp::MessageSent::decode_log(log, true)
                    .ok()
                    .map(|event| event.data.guid)
            },
            "MessageSent log missing from source receipt",
        )
        .await?;

        Ok(SentMessage {
            tx_hash,
            block,
            message_id,
        })
    })
}

async fn source_event_id_with_retry<F, Fut, D>(
    tx_hash: B256,
    retry_attempts: u64,
    retry_sleep: Duration,
    mut fetch_logs: F,
    mut decode: D,
    missing_message: &'static str,
) -> Result<B256>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SourceEventLog>>>,
    D: FnMut(&PrimitiveLog) -> Option<B256>,
{
    for attempt in 0..=retry_attempts {
        let message_id = fetch_logs()
            .await?
            .into_iter()
            .filter(|log| log.tx_hash == Some(tx_hash))
            .find_map(|log| decode(&log.log));
        if let Some(message_id) = message_id {
            return Ok(message_id);
        }
        if attempt < retry_attempts {
            tokio::time::sleep(retry_sleep).await;
        }
    }

    Err(eyre!(missing_message))
}

fn missing_layerzero_oapp(env_config: &EnvironmentConfig) -> eyre::Report {
    if !env_config.layerzero_oapp_enabled() {
        eyre!(
            "LayerZero starter OApp is disabled in config (`layerzero.oapp.enabled: false`); `make send` and `make e2e` require it to be enabled and deployed"
        )
    } else {
        eyre!(
            "missing LayerZero starter OApp deployment at `deployments.source.layerzero.exampleApp`; run `make deploy` for this environment"
        )
    }
}

fn send_ccv_message(
    msg_context: &CcvMessageContext,
    message: &str,
    gas: u128,
    finality: u32,
    executor: Address,
) -> Result<SentMessage> {
    match &msg_context.send_mode {
        CcvSendMode::RealCcip { source_example_app } => {
            // Real CCIP encodes finality and the executor at the source OnRamp via
            // the app/protocol; the --finality/--executor flags only apply to the
            // local mock send path.
            let ccip_receive_gas_limit = u32::try_from(gas)
                .map_err(|_| eyre!("CCIP receive gas limit exceeds uint32: {gas}"))?;
            send_via_example_app(
                msg_context,
                *source_example_app,
                ccip_receive_gas_limit,
                message,
            )
        }
        CcvSendMode::Mock {
            source_onramp,
            version_tag,
        } => send_via_mock_onramp(
            msg_context,
            *source_onramp,
            *version_tag,
            message,
            gas,
            finality,
            executor,
        ),
    }
}

fn send_via_example_app(
    msg_context: &CcvMessageContext,
    app_addr: Address,
    ccip_receive_gas_limit: u32,
    message: &str,
) -> Result<SentMessage> {
    let signer: PrivateKeySigner = msg_context.private_key.parse()?;
    let wallet = EthereumWallet::from(signer);
    let source_rpc = msg_context.source_rpc.clone();
    let dest_chain_selector = msg_context.dest_chain_selector;
    let payload = message.to_string();

    block_on(async move {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(source_rpc.parse()?);
        let app = ExampleCcipApp::new(app_addr, provider.clone());

        let fee = app
            .quote(dest_chain_selector, payload.clone(), ccip_receive_gas_limit)
            .call()
            .await?
            .fee;

        let pending = app
            .send(dest_chain_selector, payload, ccip_receive_gas_limit)
            .value(fee)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        let tx_hash = receipt.transaction_hash;
        let block = receipt
            .block_number
            .ok_or_else(|| eyre!("transaction receipt missing block number"))?;

        let message_id = source_event_id_with_retry(
            tx_hash,
            SOURCE_LOG_RETRY_ATTEMPTS,
            Duration::from_secs(SOURCE_LOG_RETRY_SECONDS),
            || async {
                let logs = provider
                    .get_logs(
                        &Filter::new()
                            .address(app_addr)
                            .from_block(block)
                            .to_block(block),
                    )
                    .await?;
                Ok(logs
                    .into_iter()
                    .map(|log| SourceEventLog {
                        tx_hash: log.transaction_hash,
                        log: PrimitiveLog {
                            address: log.inner.address,
                            data: log.inner.data.clone(),
                        },
                    })
                    .collect())
            },
            |log| {
                ExampleCcipApp::MessageSent::decode_log(log, true)
                    .ok()
                    .map(|event| event.data.messageId)
            },
            "ExampleCcipApp.MessageSent log missing from source receipt",
        )
        .await?;

        Ok(SentMessage {
            tx_hash,
            block,
            message_id,
        })
    })
}

fn send_via_mock_onramp(
    msg_context: &CcvMessageContext,
    onramp_addr: Address,
    version_tag: FixedBytes<4>,
    message: &str,
    gas: u128,
    finality: u32,
    executor: Address,
) -> Result<SentMessage> {
    let signer: PrivateKeySigner = msg_context.private_key.parse()?;
    let sender_address = signer.address();
    let wallet = EthereumWallet::from(signer);
    let source_rpc = msg_context.source_rpc.clone();
    let source_chain_selector = msg_context.source_chain_selector;
    let dest_chain_selector = msg_context.dest_chain_selector;
    let ccip_receive_gas_limit = u32::try_from(gas).unwrap_or(u32::MAX);
    let payload = message.to_string();

    block_on(async move {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(source_rpc.parse()?);
        let contract = MockCCIPOnRamp::new(onramp_addr, provider.clone());

        // The OnRamp assigns `nonce + 1` to this send; reuse it as the MessageV1
        // sequence number so each send produces a unique messageId.
        let sequence_number = contract.nonce().call().await?.value + 1;
        let encoded_message = build_mock_message_v1(
            source_chain_selector,
            dest_chain_selector,
            sequence_number,
            ccip_receive_gas_limit,
            finality,
            sender_address,
            &payload,
        );

        let pending = contract
            .sendMessage(dest_chain_selector, encoded_message, version_tag, executor)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        let tx_hash = receipt.transaction_hash;
        let block = receipt
            .block_number
            .ok_or_else(|| eyre!("transaction receipt missing block number"))?;

        let message_id = source_event_id_with_retry(
            tx_hash,
            SOURCE_LOG_RETRY_ATTEMPTS,
            Duration::from_secs(SOURCE_LOG_RETRY_SECONDS),
            || async {
                let logs = provider
                    .get_logs(
                        &Filter::new()
                            .address(onramp_addr)
                            .from_block(block)
                            .to_block(block),
                    )
                    .await?;
                Ok(logs
                    .into_iter()
                    .map(|log| SourceEventLog {
                        tx_hash: log.transaction_hash,
                        log: PrimitiveLog {
                            address: log.inner.address,
                            data: log.inner.data.clone(),
                        },
                    })
                    .collect())
            },
            |log| {
                MockCCIPOnRamp::CCIPMessageSent::decode_log(log, true)
                    .ok()
                    .map(|event| event.data.messageId)
            },
            "MockCCIPOnRamp.CCIPMessageSent log missing from source receipt",
        )
        .await?;

        Ok(SentMessage {
            tx_hash,
            block,
            message_id,
        })
    })
}

/// Parse the `--finality` flag into the CCIP MessageV1 wire value.
/// `None`/"finalized" → 0, "safe" → bit 16 set, a bare number N → N confirmations.
fn parse_finality_flag(value: Option<&str>) -> Result<u32> {
    match value {
        None => Ok(0),
        Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "finalized" | "final" | "0" => Ok(0),
            "safe" => Ok(0x0001_0000),
            other => other.parse::<u32>().map_err(|_| {
                eyre!("--finality must be 'finalized', 'safe', or a number, got '{raw}'")
            }),
        },
    }
}

/// Which source won the message's designated-executor address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutorChoice {
    /// `--executor` was passed explicitly.
    Explicit(Address),
    /// Defaulted from `operator.executor.address` in the environment config.
    FromEnvConfig(Address),
    /// Neither was set; falls back to the zero address (no operator
    /// self-executes it).
    Default,
}

impl ExecutorChoice {
    fn address(self) -> Address {
        match self {
            Self::Explicit(address) | Self::FromEnvConfig(address) => address,
            Self::Default => Address::ZERO,
        }
    }
}

/// Decide the message's designated executor: an explicit `--executor` value
/// always wins; otherwise fall back to `configured` (typically
/// `operator.executor.address` from the environment config), then to the
/// zero address. Pure/no I/O so the defaulting decision is unit-testable
/// independent of config loading or printing.
fn resolve_executor_choice(value: Option<&str>, configured: Option<Address>) -> Result<ExecutorChoice> {
    if let Some(raw) = value {
        return raw
            .parse::<Address>()
            .map(ExecutorChoice::Explicit)
            .map_err(|e| eyre!("--executor must be a valid address: {e}"));
    }
    Ok(match configured {
        Some(address) => ExecutorChoice::FromEnvConfig(address),
        None => ExecutorChoice::Default,
    })
}

/// Parse the `--executor` flag into the address designated as the message's
/// executor. When not given explicitly, defaults to the environment's
/// `operator.executor.address` (if configured and non-zero) so a bare `make
/// e2e`/`make send` exercises the executor operators are actually configured
/// to submit for; otherwise falls back to the zero address.
fn parse_executor_flag(value: Option<&str>, env_config: &EnvironmentConfig) -> Result<Address> {
    let choice = resolve_executor_choice(value, env_config.operator_executor_address()?)?;
    if let ExecutorChoice::FromEnvConfig(address) = choice {
        ui::info_stderr(&format!(
            "using operator executor {address} (from env config; pass --executor to override)"
        ));
    }
    Ok(choice.address())
}

/// Build a minimal CCIP MessageV1 wire blob matching the operator's decoder
/// (`operator/src/provider/ccip_message_v1.rs`). All multi-byte integers are
/// big-endian. Only the local mock send path uses this; real CCIP produces the
/// MessageV1 at its OnRamp. The `finality` u32 lands at byte offset 33, where the
/// operator's source-finality gate reads it.
fn build_mock_message_v1(
    source_chain_selector: u64,
    dest_chain_selector: u64,
    sequence_number: u64,
    ccip_receive_gas_limit: u32,
    finality: u32,
    sender: Address,
    payload: &str,
) -> Bytes {
    let data = payload.as_bytes();
    let mut buf = Vec::with_capacity(79 + 32 + data.len());
    buf.push(0x01); // version
    buf.extend_from_slice(&source_chain_selector.to_be_bytes()); // bytes 1-8
    buf.extend_from_slice(&dest_chain_selector.to_be_bytes()); // bytes 9-16
    buf.extend_from_slice(&sequence_number.to_be_bytes()); // bytes 17-24
    buf.extend_from_slice(&MOCK_EXECUTION_GAS_LIMIT.to_be_bytes()); // bytes 25-28 execution_gas_limit
    buf.extend_from_slice(&ccip_receive_gas_limit.to_be_bytes()); // bytes 29-32
    buf.extend_from_slice(&finality.to_be_bytes()); // bytes 33-36 (source-finality gate reads here)
    buf.extend_from_slice(&[0u8; 32]); // bytes 37-68 ccv_and_executor_hash (unused by the gate)
    // Dynamic fields: empty on/off-ramp, sender, receiver, dest_blob, token_transfer,
    // then the data payload. Length prefixes are u8 for addresses, u16 BE otherwise.
    buf.push(0); // on_ramp_address_length
    buf.push(0); // off_ramp_address_length
    // Sender is abi.encoded (32 bytes) on EVM sources, and the verifier's
    // forwardToVerifier rejects empty/malformed sender encodings.
    buf.push(32); // sender_length
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(sender.as_slice());
    buf.push(0); // receiver_length
    buf.extend_from_slice(&0u16.to_be_bytes()); // dest_blob_length
    buf.extend_from_slice(&0u16.to_be_bytes()); // token_transfer_length
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes()); // data_length
    buf.extend_from_slice(data);
    Bytes::from(buf)
}

fn maybe_refresh_ccv_epoch(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<()> {
    if !env_config.is_local() {
        return Ok(());
    }
    if runtime::setting(context, "CCV_AUTO_REFRESH_EPOCH").is_some_and(|value| value == "0") {
        return Ok(());
    }
    let eth = AlloyEth;
    genesis::ensure_local_epoch_fresh(context, env_config, &eth)
}

fn watch_message(
    msg_context: &MessageContext,
    target: WatchTarget,
    timeout: u64,
    json: bool,
) -> Result<WatchOutcome> {
    match msg_context {
        MessageContext::LayerZero(layerzero) => watch_message_with_verifier(
            msg_context.dest_rpc(),
            target,
            timeout,
            json,
            "destination target emitted log",
            move |dest_rpc, from_block, _message_id, _progress| {
                latest_layerzero_target_tx(dest_rpc, layerzero.destination_target, from_block)
            },
        ),
        MessageContext::ChainlinkCcv(ccv) => watch_message_with_verifier(
            msg_context.dest_rpc(),
            target,
            timeout,
            json,
            "destination offRamp executed message",
            move |dest_rpc, from_block, message_id, progress| {
                if let Some(tx_hash) = progress.submission_tx
                    && let Some(verified_tx) = ccv_execution_tx_from_receipt(
                        dest_rpc,
                        ccv.destination_offramp,
                        tx_hash,
                        message_id,
                    )?
                {
                    return Ok(Some(verified_tx));
                }

                latest_ccv_execution_tx(dest_rpc, ccv.destination_offramp, from_block, message_id)
            },
        ),
    }
}

fn watch_message_with_verifier<F>(
    dest_rpc: &str,
    target: WatchTarget,
    timeout: u64,
    json: bool,
    verified_label: &str,
    mut resolve_verified_tx: F,
) -> Result<WatchOutcome>
where
    F: FnMut(&str, u64, B256, &WatchProgress) -> Result<Option<B256>>,
{
    let client = Client::builder().timeout(Duration::from_secs(2)).build()?;
    let start = Instant::now();
    let mut last_progress = WatchProgress::default();
    let mut last_verified_tx = None;

    if !json {
        ui::info(&format!("watching message timeout={timeout}s"));
        ui::detail("message_id", target.message_id);
        ui::detail("tx", target.tx_hash);
        ui::blank();
    }

    loop {
        let elapsed = start.elapsed().as_secs();
        if elapsed >= timeout {
            let hint = if last_progress.submission_state.as_deref() == Some("Skipped") {
                let observed = target
                    .executor
                    .map(|address| format!(" (observed designated executor: {address})"))
                    .unwrap_or_default();
                format!(
                    "\nhint: the relayer reported this message as Skipped{observed} — \
                     its designated executor doesn't match any operator's configured \
                     executor. Pass EXECUTOR=<address> (matching an operator's \
                     `operator.executor.address`) to `make send`/`make e2e`, or omit it \
                     to use the configured default."
                )
            } else {
                String::new()
            };
            bail!("timed out after {timeout}s waiting for destination verification{hint}");
        }

        let progress = query_progress(&client, target.message_id, target.tx_hash);

        // The operator's reported execution_state is authoritative when
        // present — it reflects OffRamp.ExecutionStateChanged and is the same
        // signal regardless of which operator's submission tx mined. A
        // Failure here means delivery actually failed (receiver reverted), not
        // that verify-side timed out — fail fast rather than wait the full
        // window.
        if progress.execution_state.as_deref() == Some("Failure") {
            if !json {
                print_progress_changes(
                    &last_progress,
                    &progress,
                    last_verified_tx,
                    progress.delivery_tx,
                    elapsed,
                    verified_label,
                );
            }
            bail!(
                "destination execution failed (receiver reverted){}",
                progress
                    .delivery_tx
                    .map(|t| format!("; tx={t}"))
                    .unwrap_or_default()
            );
        }

        let verified_tx = if progress.execution_state.as_deref() == Some("Success") {
            progress.delivery_tx
        } else {
            resolve_verified_tx(dest_rpc, target.start_block, target.message_id, &progress)?
        };

        if !json && (progress != last_progress || verified_tx != last_verified_tx) {
            print_progress_changes(
                &last_progress,
                &progress,
                last_verified_tx,
                verified_tx,
                elapsed,
                verified_label,
            );
            last_progress = progress.clone();
            last_verified_tx = verified_tx;
        }

        if let Some(dest_tx) = verified_tx {
            if !json {
                ui::blank();
                ui::ok("message verified");
                ui::detail("message_id", target.message_id);
                ui::detail("dest_tx", dest_tx);
            }
            return Ok(WatchOutcome {
                status: "verified",
                message_id: target.message_id,
                dest_tx,
                elapsed,
            });
        }

        std::thread::sleep(Duration::from_secs(WATCH_POLL_SECONDS));
    }
}

fn query_progress(client: &Client, message_id: B256, tx_hash: B256) -> WatchProgress {
    let mut progress = WatchProgress::default();

    for port in OPERATOR_PORTS {
        let Ok(response) = client
            .get(format!(
                "http://localhost:{port}/debug/v1/messages?limit=50"
            ))
            .send()
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(body) = response.json::<OperatorMessagesResponse>() else {
            continue;
        };
        let Some(message) = body.messages.into_iter().find(|message| {
            message.metadata.message_id == message_id || message.metadata.event_tx_hash == tx_hash
        }) else {
            continue;
        };

        prefer_operator_status(&mut progress.operator_status, &message.status);
        if let Some(submission) = message.submission {
            prefer_submission(&mut progress, submission);
        }
    }

    progress
}

fn prefer_operator_status(slot: &mut Option<String>, candidate: &str) {
    let rank = match candidate {
        "Signed" => 3,
        "Processing" => 2,
        "Pending" => 1,
        _ => 0,
    };
    let current_rank = match slot.as_deref() {
        Some("Signed") => 3,
        Some("Processing") => 2,
        Some("Pending") => 1,
        Some(_) => 0,
        None => -1,
    };
    if rank > current_rank {
        *slot = Some(candidate.to_string());
    }
}

fn prefer_submission(progress: &mut WatchProgress, submission: OperatorSubmission) {
    let rank = match submission.state.as_str() {
        "Confirmed" => 3,
        "Submitted" => 2,
        "Failed" => 1,
        _ => 0,
    };
    let current_rank = match progress.submission_state.as_deref() {
        Some("Confirmed") => 3,
        Some("Submitted") => 2,
        Some("Failed") => 1,
        Some(_) => 0,
        None => -1,
    };
    if rank > current_rank {
        progress.submission_state = Some(submission.state);
        progress.submission_tx = submission.tx_hash;
        progress.submission_error = submission.last_error;
    }
    // execution_state is authoritative across operators — once any operator
    // observes Success/Failure on-chain, that's the final word; keep the first
    // terminal value we see.
    if let Some(state) = submission.execution_state {
        if progress.execution_state.is_none() {
            progress.execution_state = Some(state);
        }
        if progress.delivery_tx.is_none() {
            progress.delivery_tx = submission.delivery_tx_hash;
        }
    }
}

fn print_progress_changes(
    previous: &WatchProgress,
    current: &WatchProgress,
    previous_verified_tx: Option<B256>,
    verified_tx: Option<B256>,
    elapsed: u64,
    verified_label: &str,
) {
    let prefix = format!("+{elapsed}s");
    if current.operator_status != previous.operator_status
        && let Some(status) = current.operator_status.as_deref()
    {
        println!("{prefix} {}", format_operator_status(status));
    }
    if (current.submission_state != previous.submission_state
        || current.submission_tx != previous.submission_tx)
        && let Some(state) = current.submission_state.as_deref()
    {
        println!(
            "{prefix} {}",
            format_submission_status(state, current.submission_tx)
        );
    }
    if current.submission_error != previous.submission_error
        && current.submission_state.as_deref() == Some("Failed")
        && let Some(error) = current.submission_error.as_deref()
    {
        println!("{prefix} relayer error: {error}");
    }
    // Surface the on-chain execution state transition once. This is distinct
    // from the per-operator submission state — it answers "did the message
    // actually deliver?" regardless of whose tx mined.
    if current.execution_state != previous.execution_state
        && let Some(state) = current.execution_state.as_deref()
    {
        println!(
            "{prefix} {}",
            format_execution_state(state, current.delivery_tx)
        );
    }
    if verified_tx != previous_verified_tx
        && let Some(dest_tx) = verified_tx
    {
        println!("{prefix} {verified_label} tx={dest_tx}");
    }
}

fn format_execution_state(state: &str, delivery_tx: Option<B256>) -> String {
    match state {
        "Success" => delivery_tx
            .map(|tx| format!("On-chain: delivered (tx: {tx})"))
            .unwrap_or_else(|| "On-chain: delivered".to_string()),
        "Failure" => delivery_tx
            .map(|tx| format!("On-chain: execution failed — receiver reverted (tx: {tx})"))
            .unwrap_or_else(|| "On-chain: execution failed — receiver reverted".to_string()),
        other => format!("On-chain: {other}"),
    }
}

fn format_operator_status(status: &str) -> String {
    match status {
        "Pending" => "Operators: waiting to batch".to_string(),
        "Processing" => "Operators: collecting BLS signatures".to_string(),
        "Signed" => "Operators: signed (quorum reached)".to_string(),
        other => format!("Operators: {other}"),
    }
}

fn format_submission_status(state: &str, tx_hash: Option<B256>) -> String {
    match state {
        "Pending" => "Relayer: queued".to_string(),
        "Submitted" => "Relayer: submitted".to_string(),
        "Confirmed" => tx_hash
            .map(|hash| format!("Relayer: confirmed (tx: {hash})"))
            .unwrap_or_else(|| "Relayer: confirmed".to_string()),
        "Failed" => "Relayer: failed".to_string(),
        other => format!("Relayer: {other}"),
    }
}

/// `from_block` is the source-chain send-tx block; on testnet it can exceed
/// the destination's `latest` (different chains, different heights), which
/// makes eth_getLogs return empty. Fall back to a recent-blocks window.
fn clamp_from_block(from_block: u64, latest: u64) -> u64 {
    let recent = latest.saturating_sub(MAX_LOG_BLOCK_RANGE);
    if from_block > latest {
        recent
    } else {
        from_block.max(recent)
    }
}

fn latest_layerzero_target_tx(
    dest_rpc: &str,
    target: Address,
    from_block: u64,
) -> Result<Option<B256>> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(dest_rpc.parse()?);
        let latest = provider.get_block_number().await?;
        let safe_from = clamp_from_block(from_block, latest);
        let logs = provider
            .get_logs(&Filter::new().address(target).from_block(safe_from))
            .await?;
        Ok(logs.last().and_then(|log| log.transaction_hash))
    })
}

fn latest_ccv_execution_tx(
    dest_rpc: &str,
    off_ramp: Address,
    from_block: u64,
    message_id: B256,
) -> Result<Option<B256>> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(dest_rpc.parse()?);
        let latest = provider.get_block_number().await?;
        let safe_from = clamp_from_block(from_block, latest);
        let logs = provider
            .get_logs(&Filter::new().address(off_ramp).from_block(safe_from))
            .await?;
        let topic0 = B256::from(keccak256(CCV_MESSAGE_EXECUTED_EVENT.as_bytes()));
        Ok(logs.into_iter().rev().find_map(|log| {
            let tx_hash = log.transaction_hash?;
            ccv_message_executed_log_matches(&log.inner, off_ramp, message_id, topic0)
                .then_some(tx_hash)
        }))
    })
}

fn ccv_execution_tx_from_receipt(
    dest_rpc: &str,
    off_ramp: Address,
    tx_hash: B256,
    message_id: B256,
) -> Result<Option<B256>> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(dest_rpc.parse()?);
        let Some(receipt) = provider.get_transaction_receipt(tx_hash).await? else {
            return Ok(None);
        };
        let topic0 = B256::from(keccak256(CCV_MESSAGE_EXECUTED_EVENT.as_bytes()));
        Ok(receipt
            .inner
            .logs()
            .iter()
            .any(|log| ccv_message_executed_log_matches(log.as_ref(), off_ramp, message_id, topic0))
            .then_some(tx_hash))
    })
}

fn ccv_message_executed_log_matches(
    log: &PrimitiveLog,
    off_ramp: Address,
    message_id: B256,
    topic0: B256,
) -> bool {
    if log.address != off_ramp {
        return false;
    }

    let topics = log.data.topics();
    let matches_event = topics.first().is_some_and(|value| *value == topic0);
    let matches_message = topics.get(1).is_some_and(|value| *value == message_id);
    matches_event && matches_message
}

fn resolve_watch_target(
    context: &ResolvedContext,
    dest_rpc: &str,
    args: &MsgWatchArgs,
) -> Result<WatchTarget> {
    let cache = load_cache(context).ok();
    let tx_hash = match args.tx.as_deref() {
        Some(value) => value.parse()?,
        None => cache
            .as_ref()
            .map(|cache| cache.tx_hash)
            .ok_or_else(|| eyre!("no cached message; run `send` first or pass --tx"))?,
    };
    let message_id = match args.id.as_deref() {
        Some(value) => value.parse()?,
        None => cache
            .as_ref()
            .map(|cache| cache.message_id)
            .ok_or_else(|| eyre!("no cached message; run `send` first or pass --id"))?,
    };

    let current_head = AlloyEth.block_number(dest_rpc).unwrap_or(0);
    let executor = cache.as_ref().and_then(|cache| cache.executor);
    let start_block = cache
        .map(|cache| cache.block.min(current_head))
        .unwrap_or(current_head);

    Ok(WatchTarget {
        tx_hash,
        message_id,
        start_block,
        executor,
    })
}

fn wait_for_message_readiness(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    source_rpc: &str,
) -> Result<()> {
    if !env_config.is_local() {
        return Ok(());
    }

    let cursor_file = context
        .project_root
        .join("data")
        .join("oz-monitor")
        .join("local_anvil_last_block.txt");
    let deadline = Instant::now() + Duration::from_secs(MESSAGE_READY_TIMEOUT_SECONDS);
    let mut step = ui::step("wait for monitor catch-up");

    loop {
        let head = AlloyEth.block_number(source_rpc).ok();
        let cursor = fs::read_to_string(&cursor_file)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());

        if let (Some(head), Some(cursor)) = (head, cursor) {
            let lag = head.saturating_sub(cursor.min(head));
            if lag <= MESSAGE_READY_MAX_LAG_BLOCKS {
                step.done("monitor caught up");
                return Ok(());
            }
            step.heartbeat_with(&format!("monitor lag is {lag} block(s)"));
        }

        if Instant::now() >= deadline {
            bail!(
                "monitor did not catch up within {}s; run `make status ENV={}` to inspect lag",
                MESSAGE_READY_TIMEOUT_SECONDS,
                context.env_name,
            );
        }

        std::thread::sleep(Duration::from_secs(MESSAGE_READY_POLL_SECONDS));
    }
}

fn cache_file(context: &ResolvedContext) -> std::path::PathBuf {
    context.generated_dir.join("msg-cache.json")
}

fn save_cache(context: &ResolvedContext, cache: &MessageCache) -> Result<()> {
    fs::create_dir_all(&context.generated_dir)?;
    fs::write(
        cache_file(context),
        format!("{}\n", serde_json::to_string_pretty(cache)?),
    )?;
    Ok(())
}

fn load_cache(context: &ResolvedContext) -> Result<MessageCache> {
    let path = cache_file(context);
    let body = fs::read_to_string(&path)
        .map_err(|err| eyre!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&body).map_err(|err| eyre!("failed to parse {}: {err}", path.display()))
}

fn block_on<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}

fn load_deployments_or_bail(context: &ResolvedContext) -> Result<DeploymentsConfig> {
    let deployments = DeploymentsConfig::load(&context.deployments).map_err(|_| {
        eyre!(
            "no deployment state found at {}. Run `make start` first.",
            context.deployments.display()
        )
    })?;
    if !deployments.role_has_entries(ChainRole::Source)
        || !deployments.role_has_entries(ChainRole::Destination)
    {
        bail!(
            "incomplete deployment state in {}. Run `make start` first.",
            context.deployments.display()
        );
    }
    Ok(deployments)
}

/// Quick health check before e2e: verify operators are reachable.
fn preflight_check(context: &ResolvedContext) -> Result<()> {
    let client = Client::builder().timeout(Duration::from_secs(3)).build()?;

    let mut failures = Vec::new();
    for i in 1..=3 {
        let url = format!("http://localhost:{}/healthz", OPERATOR_PORTS[i - 1]);
        let healthy = client
            .get(&url)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if !healthy {
            failures.push(format!("operator-{i}"));
        }
    }

    if !failures.is_empty() {
        let hint = if runtime::setting(context, "SOURCE_RPC")
            .filter(|v| !v.is_empty())
            .is_none()
        {
            "make start"
        } else {
            &format!("make start ENV={}", context.env_name)
        };
        bail!(
            "stack not ready ({} unreachable). Run `{}` first.",
            failures.join(", "),
            hint
        );
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn test_context() -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        std::mem::forget(temp_dir); // keep temp dir alive for test duration
        ResolvedContext {
            project_root: root.clone(),
            env_name: "local".to_string(),
            env_config: root.join("local.json"),
            deployments: root.join("deployments.json"),
            generated_dir: root.join("generated").join("local"),
        }
    }

    /// Local Anvil deploys mocks and no Router — load_ccv_context must pick
    /// the Mock send mode using `chainlinkCcv.onRamp` from deployments.
    #[test]
    fn load_ccv_context_picks_mock_mode_for_local_env() {
        use std::fs;
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let env_path = root.join("local.json");
        let deployments_path = root.join("deployments.json");
        fs::write(
            &env_path,
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                },
                "funding": {
                    "operatorAmountWei": "1000000000000000000",
                    "signerAmountWei": "1000000000000000000",
                    "minBalanceThresholdWei": "1000000000000000000"
                }
            }"#,
        )
        .unwrap();
        fs::write(
            &deployments_path,
            r#"{
                "source": { "chainlinkCcv": { "onRamp": "0x1111111111111111111111111111111111111111" } },
                "destination": { "chainlinkCcv": { "offRamp": "0x2222222222222222222222222222222222222222" } }
            }"#,
        )
        .unwrap();
        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "local".to_string(),
            env_config: env_path,
            deployments: deployments_path,
            generated_dir: root.join("generated").join("local"),
        };
        std::mem::forget(tmp);

        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();
        let deployments = DeploymentsConfig::load(&context.deployments).unwrap();
        let runtime = RuntimeInputs {
            source_rpc: Some("http://localhost:8545".to_string()),
            dest_rpc: Some("http://localhost:8546".to_string()),
            private_key: Some(
                "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string(),
            ),
        };

        let ctx = load_ccv_context(&context, &env_config, &deployments, &runtime).unwrap();
        match ctx.send_mode {
            CcvSendMode::Mock { source_onramp, .. } => {
                assert_eq!(
                    source_onramp,
                    "0x1111111111111111111111111111111111111111"
                        .parse::<Address>()
                        .unwrap()
                );
            }
            other => panic!("expected Mock send mode for local env, got {other:?}"),
        }
    }

    #[test]
    fn cache_round_trip_uses_generated_dir() {
        let context = test_context();
        let cache = MessageCache {
            tx_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
            block: 42,
            message_id: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .parse()
                .unwrap(),
            message: "hello".to_string(),
            executor: None,
        };

        save_cache(&context, &cache).unwrap();
        let loaded = load_cache(&context).unwrap();

        assert_eq!(loaded.tx_hash, cache.tx_hash);
        assert_eq!(loaded.message_id, cache.message_id);
        assert!(cache_file(&context).ends_with("generated/local/msg-cache.json"));
    }

    #[test]
    fn source_event_id_retries_until_source_logs_are_indexed() {
        let tx_hash: B256 = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap();
        let message_id: B256 = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .unwrap();
        let source_address = Address::repeat_byte(0x11);
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));

        let result = block_on(source_event_id_with_retry(
            tx_hash,
            2,
            Duration::ZERO,
            {
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        let attempt = calls.get();
                        calls.set(attempt + 1);
                        if attempt == 0 {
                            return Ok(vec![]);
                        }

                        Ok(vec![SourceEventLog {
                            tx_hash: Some(tx_hash),
                            log: PrimitiveLog {
                                address: source_address,
                                data: alloy::primitives::LogData::new_unchecked(
                                    Vec::new(),
                                    Bytes::new(),
                                ),
                            },
                        }])
                    }
                }
            },
            move |log| (log.address == source_address).then_some(message_id),
            "source log missing",
        ))
        .unwrap();

        assert_eq!(result, message_id);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn ccv_message_executed_log_match_requires_offramp_and_message_id() {
        let off_ramp: Address = "0x0ed64d01d0b4b655e410ef1441dd677b695639e7"
            .parse()
            .unwrap();
        let message_id: B256 = "0xf7baf63ba6694dc9e1832e334c558f532b70e525a8b2fd4a832365035c8c5c1c"
            .parse()
            .unwrap();
        let topic0 = B256::from(keccak256(CCV_MESSAGE_EXECUTED_EVENT.as_bytes()));
        let good = PrimitiveLog {
            address: off_ramp,
            data: alloy::primitives::LogData::new_unchecked(vec![topic0, message_id], Bytes::new()),
        };
        let wrong_address = PrimitiveLog {
            address: Address::repeat_byte(0x11),
            data: good.data.clone(),
        };
        let wrong_message = PrimitiveLog {
            address: off_ramp,
            data: alloy::primitives::LogData::new_unchecked(
                vec![
                    topic0,
                    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .parse()
                        .unwrap(),
                ],
                Bytes::new(),
            ),
        };

        assert!(ccv_message_executed_log_matches(
            &good, off_ramp, message_id, topic0
        ));
        assert!(!ccv_message_executed_log_matches(
            &wrong_address,
            off_ramp,
            message_id,
            topic0
        ));
        assert!(!ccv_message_executed_log_matches(
            &wrong_message,
            off_ramp,
            message_id,
            topic0
        ));
    }

    #[test]
    fn clamp_from_block_falls_back_when_above_latest() {
        let latest = 10_000_000;
        let recent = latest - MAX_LOG_BLOCK_RANGE;
        assert_eq!(clamp_from_block(41_000_000, latest), recent);
    }

    #[test]
    fn clamp_from_block_passes_through_when_in_range() {
        let latest = 1_000;
        let recent = latest - MAX_LOG_BLOCK_RANGE;
        assert_eq!(clamp_from_block(500, latest), 500.max(recent));
        assert_eq!(clamp_from_block(latest, latest), latest);
    }

    #[test]
    fn clamp_from_block_floors_at_recent_window() {
        let latest = 1_000;
        let recent = latest - MAX_LOG_BLOCK_RANGE;
        assert_eq!(clamp_from_block(0, latest), recent);
    }

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    #[test]
    fn resolve_executor_choice_prefers_explicit_flag_over_configured_default() {
        let explicit = addr("0x1111111111111111111111111111111111111111");
        let configured = addr("0x2222222222222222222222222222222222222222");
        let choice = resolve_executor_choice(
            Some("0x1111111111111111111111111111111111111111"),
            Some(configured),
        )
        .unwrap();
        assert_eq!(choice, ExecutorChoice::Explicit(explicit));
        assert_eq!(choice.address(), explicit);
    }

    #[test]
    fn resolve_executor_choice_falls_back_to_env_config_when_flag_absent() {
        let configured = addr("0x2222222222222222222222222222222222222222");
        let choice = resolve_executor_choice(None, Some(configured)).unwrap();
        assert_eq!(choice, ExecutorChoice::FromEnvConfig(configured));
        assert_eq!(choice.address(), configured);
    }

    #[test]
    fn resolve_executor_choice_defaults_to_zero_when_nothing_set() {
        let choice = resolve_executor_choice(None, None).unwrap();
        assert_eq!(choice, ExecutorChoice::Default);
        assert_eq!(choice.address(), Address::ZERO);
    }

    #[test]
    fn resolve_executor_choice_rejects_invalid_explicit_address() {
        let err = resolve_executor_choice(Some("not-an-address"), None).unwrap_err();
        assert!(err.to_string().contains("--executor must be a valid address"));
    }
}
