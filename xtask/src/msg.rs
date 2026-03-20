use std::fs;
use std::time::{Duration, Instant};

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256, Bytes, Log as PrimitiveLog, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::SolEvent;
use eyre::{Result, bail, eyre};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::cli::{MsgArgs, MsgCommand, MsgE2eArgs, MsgSendArgs, MsgWatchArgs};
use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::runtime::RuntimeInputs;

const MESSAGE_READY_TIMEOUT_SECONDS: u64 = 180;
const MESSAGE_READY_POLL_SECONDS: u64 = 2;
const MESSAGE_READY_MAX_LAG_BLOCKS: u64 = 20;
const WATCH_POLL_SECONDS: u64 = 2;
const OPERATOR_PORTS: [u16; 3] = [3001, 3002, 3003];

sol! {
    struct MessagingFee {
        uint256 nativeFee;
        uint256 lzTokenFee;
    }

    #[sol(rpc)]
    interface TestOApp {
        event MessageSent(uint32 indexed dstEid, string message, bytes32 guid, uint64 nonce);
        function buildOptions(uint128 _gas) external pure returns (bytes memory options);
        function quote(uint32 _dstEid, string calldata _message, bytes calldata _options, bool _payInLzToken)
            external
            view
            returns (MessagingFee memory fee);
        function send(uint32 _dstEid, string calldata _message, bytes calldata _options) external payable;
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

pub fn run_command(context: &ResolvedContext, args: &MsgArgs) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    if env_config.active_provider != "layerzero" {
        bail!(
            "xtask msg currently supports layerzero only; active provider is {}",
            env_config.active_provider
        );
    }

    let deployments = DeploymentsConfig::load(&context.deployments)?;
    let runtime = RuntimeInputs::resolve(context, &env_config);
    let msg_context = load_layerzero_context(&env_config, &deployments, &runtime)?;

    match &args.command {
        MsgCommand::Send(send) => run_send(context, &msg_context, send),
        MsgCommand::Watch(watch) => run_watch_command(context, &env_config, &msg_context, watch),
        MsgCommand::E2e(e2e) => run_e2e(context, &env_config, &msg_context, e2e),
    }
}

fn run_send(context: &ResolvedContext, msg_context: &LayerZeroMessageContext, args: &MsgSendArgs) -> Result<()> {
    let sent = send_message(msg_context, &args.message, args.gas)?;
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
                "provider": "layerzero",
                "tx_hash": sent.tx_hash,
                "block": sent.block,
                "message_id": sent.message_id,
            }))?
        );
    } else {
        println!("Provider: layerzero");
        println!("Sending message: {:?}", args.message);
        println!("Message ID: {}", sent.message_id);
        println!("TX: {}", sent.tx_hash);
        println!("Block: {}", sent.block);
        println!();
        println!("Track with: make -f Makefile.xtask watch");
    }

    Ok(())
}

fn run_watch_command(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &LayerZeroMessageContext,
    args: &MsgWatchArgs,
) -> Result<()> {
    wait_for_message_readiness(context, env_config, &msg_context.source_rpc)?;
    let target = resolve_watch_target(context, msg_context, args)?;
    let outcome = watch_message(msg_context, target, args.timeout, args.json)?;

    if args.json {
        println!("{}", serde_json::to_string(&outcome)?);
    }

    Ok(())
}

fn run_e2e(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    msg_context: &LayerZeroMessageContext,
    args: &MsgE2eArgs,
) -> Result<()> {
    let sent = send_message(msg_context, &args.message, args.gas)?;
    save_cache(
        context,
        &MessageCache {
            tx_hash: sent.tx_hash,
            block: sent.block,
            message_id: sent.message_id,
            message: args.message.clone(),
        },
    )?;

    wait_for_message_readiness(context, env_config, &msg_context.source_rpc)?;
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
                "provider": "layerzero",
                "tx_hash": sent.tx_hash,
                "block": sent.block,
                "message_id": sent.message_id,
                "watch": outcome,
            }))?
        );
    }

    Ok(())
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
        .deployment(ChainRole::Source, "testOApp")
        .and_then(|value| parse_address(&value))
        .ok_or_else(|| eyre!("missing source TestOApp deployment"))?;
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

fn send_message(msg_context: &LayerZeroMessageContext, message: &str, gas: u128) -> Result<SentMessage> {
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
        let contract = TestOApp::new(source_oapp, provider.clone());

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
            .get_logs(&Filter::new().address(source_oapp).from_block(block).to_block(block))
            .await?;
        let message_id = logs
            .into_iter()
            .filter(|log| log.transaction_hash == Some(tx_hash))
            .find_map(|log| {
                let primitive_log = PrimitiveLog {
                    address: log.inner.address,
                    data: log.inner.data.clone(),
                };
                TestOApp::MessageSent::decode_log(&primitive_log, true).ok()
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

fn watch_message(
    msg_context: &LayerZeroMessageContext,
    target: WatchTarget,
    timeout: u64,
    json: bool,
) -> Result<WatchOutcome> {
    let client = Client::builder().timeout(Duration::from_secs(2)).build()?;
    let start = Instant::now();
    let mut last_progress = WatchProgress::default();
    let mut last_verified_tx = None;

    if !json {
        println!("Watching LayerZero message (timeout: {timeout}s)");
        println!("Message ID: {}", target.message_id);
        println!("TX: {}", target.tx_hash);
        println!();
    }

    loop {
        let elapsed = start.elapsed().as_secs();
        if elapsed >= timeout {
            bail!("timed out after {timeout}s waiting for destination verification");
        }

        let progress = query_progress(&client, target.message_id, target.tx_hash);
        let verified_tx = latest_target_tx(
            &msg_context.dest_rpc,
            msg_context.destination_target,
            target.start_block,
        )?;

        if !json && (progress != last_progress || verified_tx != last_verified_tx) {
            print_progress_changes(&last_progress, &progress, last_verified_tx, verified_tx);
            last_progress = progress.clone();
            last_verified_tx = verified_tx;
        }

        if let Some(dest_tx) = verified_tx {
            if !json {
                println!();
                println!("Message verified on destination chain");
                println!("Message ID: {}", target.message_id);
                println!("Dest TX: {dest_tx}");
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
            .get(format!("http://localhost:{port}/debug/v1/messages?limit=50"))
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
        let Some(message) = body
            .messages
            .into_iter()
            .find(|message| {
                message.metadata.message_id == message_id || message.metadata.event_tx_hash == tx_hash
            })
        else {
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
) {
    let timestamp = chrono_like_timestamp();
    if current.operator_status != previous.operator_status
        && let Some(status) = current.operator_status.as_deref()
    {
        println!("[{timestamp}] {}", format_operator_status(status));
    }
    if (current.submission_state != previous.submission_state
        || current.submission_tx != previous.submission_tx)
        && let Some(state) = current.submission_state.as_deref()
    {
        println!(
            "[{timestamp}] {}",
            format_submission_status(state, current.submission_tx)
        );
    }
    if current.submission_error != previous.submission_error
        && current.submission_state.as_deref() == Some("Failed")
        && let Some(error) = current.submission_error.as_deref()
    {
        println!("[{timestamp}] Relayer error: {error}");
    }
    if verified_tx != previous_verified_tx
        && let Some(dest_tx) = verified_tx
    {
        println!("[{timestamp}] Destination target emitted log (tx: {dest_tx})");
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

fn latest_target_tx(dest_rpc: &str, target: Address, from_block: u64) -> Result<Option<B256>> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(dest_rpc.parse()?);
        let logs = provider
            .get_logs(&Filter::new().address(target).from_block(from_block))
            .await?;
        Ok(logs.last().and_then(|log| log.transaction_hash))
    })
}

fn resolve_watch_target(
    context: &ResolvedContext,
    msg_context: &LayerZeroMessageContext,
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

    let current_head = AlloyEth.block_number(&msg_context.dest_rpc).unwrap_or(0);
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

    loop {
        let head = AlloyEth.block_number(source_rpc).ok();
        let cursor = fs::read_to_string(&cursor_file)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());

        if let (Some(head), Some(cursor)) = (head, cursor) {
            let lag = head.saturating_sub(cursor.min(head));
            if lag <= MESSAGE_READY_MAX_LAG_BLOCKS {
                return Ok(());
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "monitor did not catch up within {}s; run `make -f Makefile.xtask status` to inspect lag",
                MESSAGE_READY_TIMEOUT_SECONDS
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

fn chrono_like_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now % 86_400;
    let hour = secs / 3_600;
    let minute = (secs % 3_600) / 60;
    let second = secs % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn test_context() -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        std::mem::forget(temp_dir);
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
}
