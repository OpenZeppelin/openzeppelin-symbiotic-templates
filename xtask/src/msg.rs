use std::fs;
use std::time::{Duration, Instant};

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256, Bytes, FixedBytes, Log as PrimitiveLog, U256, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::{SolEvent, SolValue};
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
const WATCH_POLL_SECONDS: u64 = 2;
const MAX_LOG_BLOCK_RANGE: u64 = 10;
const OPERATOR_PORTS: [u16; 3] = [3001, 3002, 3003];
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

    struct CcvReceipt {
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
            CcvReceipt[] receipts,
            bytes[] verifierBlobs
        );
        function sendMessage(uint64 destChainSelector, bytes calldata encodedMessage, bytes4 versionTag)
            external
            returns (bytes32 messageId);
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
    source_onramp: Address,
    destination_offramp: Address,
    dest_chain_selector: u64,
    version_tag: FixedBytes<4>,
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
struct WatchTarget {
    tx_hash: B256,
    message_id: B256,
    start_block: u64,
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
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct WatchProgress {
    operator_status: Option<String>,
    submission_state: Option<String>,
    submission_tx: Option<B256>,
    submission_error: Option<String>,
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
    let sent = send_message(context, env_config, msg_context, &args.message, args.gas)?;
    save_cache(
        context,
        &MessageCache {
            tx_hash: sent.tx_hash,
            block: sent.block,
            message_id: sent.message_id,
            message: args.message.clone(),
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
    let sent = send_message(context, env_config, msg_context, &args.message, args.gas)?;
    save_cache(
        context,
        &MessageCache {
            tx_hash: sent.tx_hash,
            block: sent.block,
            message_id: sent.message_id,
            message: args.message.clone(),
        },
    )?;

    wait_for_message_readiness(context, env_config, msg_context.source_rpc())?;
    let outcome = watch_message(
        msg_context,
        WatchTarget {
            tx_hash: sent.tx_hash,
            message_id: sent.message_id,
            start_block: sent.block,
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
        .deployment(ChainRole::Destination, "dvn")
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
    let source_onramp = runtime::setting(context, "CCV_SOURCE_ONRAMP_ADDRESS")
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp"))
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| eyre!("missing source CCV onRamp deployment"))?;
    let destination_offramp = runtime::setting(context, "CCV_DEST_OFFRAMP_ADDRESS")
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp"))
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| eyre!("missing destination CCV offRamp deployment"))?;
    let dest_chain_selector = runtime::setting(context, "CCV_DEST_CHAIN_SELECTOR")
        .unwrap_or_else(|| env_config.chains.destination.chain_id.to_string())
        .parse()?;
    let version_tag = runtime::setting(context, "CCV_VERSION_TAG")
        .unwrap_or_else(|| DEFAULT_CCV_VERSION_TAG.to_string())
        .parse()?;

    Ok(CcvMessageContext {
        source_rpc,
        dest_rpc,
        private_key,
        source_onramp,
        destination_offramp,
        dest_chain_selector,
        version_tag,
    })
}

fn send_message(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &MessageContext,
    message: &str,
    gas: u128,
) -> Result<SentMessage> {
    match msg_context {
        MessageContext::LayerZero(layerzero) => send_layerzero_message(layerzero, message, gas),
        MessageContext::ChainlinkCcv(ccv) => {
            maybe_refresh_ccv_epoch(context, env_config)?;
            send_ccv_message(ccv, message)
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

        let logs = provider
            .get_logs(
                &Filter::new()
                    .address(source_oapp)
                    .from_block(block)
                    .to_block(block),
            )
            .await?;
        let message_id = logs
            .into_iter()
            .filter(|log| log.transaction_hash == Some(tx_hash))
            .find_map(|log| {
                let primitive_log = PrimitiveLog {
                    address: log.inner.address,
                    data: log.inner.data.clone(),
                };
                ExampleOApp::MessageSent::decode_log(&primitive_log, true).ok()
            })
            .map(|event| event.data.guid)
            .ok_or_else(|| eyre!("MessageSent log missing from source receipt"))?;

        Ok(SentMessage {
            tx_hash,
            block,
            message_id,
        })
    })
}

fn missing_layerzero_oapp(env_config: &EnvironmentConfig) -> eyre::Report {
    if !env_config.layerzero_oapp_enabled() {
        eyre!(
            "LayerZero starter OApp is disabled in config (`layerzero.oapp.enabled: false`); `make send` and `make e2e` require it to be enabled and deployed"
        )
    } else {
        eyre!(
            "missing LayerZero starter OApp deployment at `deployments.layerzero.oapp.source`; run `make deploy` for this environment"
        )
    }
}

fn send_ccv_message(msg_context: &CcvMessageContext, message: &str) -> Result<SentMessage> {
    let signer: PrivateKeySigner = msg_context.private_key.parse()?;
    let wallet = EthereumWallet::from(signer);
    let source_rpc = msg_context.source_rpc.clone();
    let source_onramp = msg_context.source_onramp;
    let dest_chain_selector = msg_context.dest_chain_selector;
    let version_tag = msg_context.version_tag;
    let encoded_message = Bytes::from(message.to_string().abi_encode());

    block_on(async move {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(source_rpc.parse()?);
        let contract = MockCCIPOnRamp::new(source_onramp, provider.clone());

        let pending = contract
            .sendMessage(dest_chain_selector, encoded_message, version_tag)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        let tx_hash = receipt.transaction_hash;
        let block = receipt
            .block_number
            .ok_or_else(|| eyre!("transaction receipt missing block number"))?;

        let logs = provider
            .get_logs(
                &Filter::new()
                    .address(source_onramp)
                    .from_block(block)
                    .to_block(block),
            )
            .await?;
        let message_id = logs
            .into_iter()
            .filter(|log| log.transaction_hash == Some(tx_hash))
            .find_map(|log| {
                let primitive_log = PrimitiveLog {
                    address: log.inner.address,
                    data: log.inner.data.clone(),
                };
                MockCCIPOnRamp::CCIPMessageSent::decode_log(&primitive_log, true).ok()
            })
            .map(|event| event.data.messageId)
            .ok_or_else(|| eyre!("CCIPMessageSent log missing from source receipt"))?;

        Ok(SentMessage {
            tx_hash,
            block,
            message_id,
        })
    })
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
            bail!("timed out after {timeout}s waiting for destination verification");
        }

        let progress = query_progress(&client, target.message_id, target.tx_hash);
        let verified_tx =
            resolve_verified_tx(dest_rpc, target.start_block, target.message_id, &progress)?;

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
    if verified_tx != previous_verified_tx
        && let Some(dest_tx) = verified_tx
    {
        println!("{prefix} {verified_label} tx={dest_tx}");
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

fn latest_layerzero_target_tx(
    dest_rpc: &str,
    target: Address,
    from_block: u64,
) -> Result<Option<B256>> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(dest_rpc.parse()?);
        let latest = provider.get_block_number().await?;
        let safe_from = from_block.max(latest.saturating_sub(MAX_LOG_BLOCK_RANGE));
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
        let safe_from = from_block.max(latest.saturating_sub(MAX_LOG_BLOCK_RANGE));
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
    let start_block = cache
        .map(|cache| cache.block.min(current_head))
        .unwrap_or(current_head);

    Ok(WatchTarget {
        tx_hash,
        message_id,
        start_block,
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
        };

        save_cache(&context, &cache).unwrap();
        let loaded = load_cache(&context).unwrap();

        assert_eq!(loaded.tx_hash, cache.tx_hash);
        assert_eq!(loaded.message_id, cache.message_id);
        assert!(cache_file(&context).ends_with("generated/local/msg-cache.json"));
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
}
