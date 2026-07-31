use std::process::Command;
use std::thread;
use std::time::Duration;

use alloy::primitives::Address;
use eyre::{Result, bail, eyre};
use serde::Deserialize;

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::runtime;
use crate::ui;

const GENESIS_READY_TIMEOUT_SECONDS: u64 = 900;
const GENESIS_READY_POLL_SECONDS: u64 = 10;
/// Number of consecutive polls the total voting power must stay unchanged
/// before genesis is treated as ready. Defends against the race where the
/// first operator's stake activates and trips the quorum threshold alone
/// (totalVP >= ⅔ × totalVP is self-satisfying), committing a snapshot that
/// omits later-activating operators.
const GENESIS_STABLE_POLLS: usize = 3;

/// Must match REQUIRED_KEY_TAG_BLS / REQUIRED_KEY_TAG_SECONDARY_BLS in DeployRelayInfra.s.sol
const REQUIRED_KEY_TAGS: [u8; 2] = [15, 11];
const PREFLIGHT_RETRY_DELAY: Duration = Duration::from_secs(5);
const PREFLIGHT_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct RelayInfraAddresses {
    pub driver: String,
    pub settlement: String,
}

#[derive(Debug, Deserialize)]
struct GenesisPreview {
    header: GenesisHeader,
}

#[derive(Debug, Deserialize)]
struct GenesisHeader {
    #[serde(rename = "quorumThreshold")]
    quorum_threshold: u128,
    #[serde(rename = "totalVotingPower")]
    total_voting_power: u128,
}

pub fn ensure_genesis_for_relay(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    relay: &RelayInfraAddresses,
    force: bool,
    fund_keys: bool,
) -> Result<()> {
    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    let driver_address = relay.driver.to_ascii_lowercase();
    let settlement_address = relay.settlement.clone();
    let dest_rpc = runtime
        .dest_rpc
        .clone()
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;
    let private_key = runtime
        .private_key
        .clone()
        .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?;

    if !force && genesis_exists(&settlement_address, &dest_rpc)? {
        if genesis_is_stale(context, &settlement_address, &dest_rpc, 0)? {
            ui::warn("committed settlement genesis is stale; refreshing");
        } else {
            verify_genesis(&settlement_address, &dest_rpc)?;
            ui::ok("genesis already committed");
            return Ok(());
        }
    }

    if fund_keys {
        fund_relay_keys(context, env_config, &dest_rpc, &private_key)?;
    }

    let genesis_key =
        runtime::setting(context, "GENESIS_PRIVATE_KEY").unwrap_or_else(|| private_key.clone());
    let relay_image = runtime::setting(context, "RELAY_IMAGE")
        .unwrap_or_else(|| "symbioticfi/relay:1.1.1".to_string());

    if env_config.is_local() {
        let network_name = local_bridge_network()?;
        let secret_keys = format!(
            "{}:{},{}:{}",
            env_config.chains.source.chain_id,
            genesis_key,
            env_config.chains.destination.chain_id,
            genesis_key
        );

        let preview_args = genesis_command_args(
            "http://anvil:8545,http://anvil-settlement:8546",
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            Some(network_name),
            &secret_keys,
            None,
            false,
        );
        wait_for_genesis_ready(&preview_args)?;

        let commit_args = genesis_command_args(
            "http://anvil:8545,http://anvil-settlement:8546",
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            Some(local_bridge_network()?),
            &secret_keys,
            None,
            true,
        );
        run_status("docker", &commit_args)?;
    } else {
        wait_for_contract(&dest_rpc, &driver_address, "driver")?;

        let deployments = DeploymentsConfig::load_or_default(&context.deployments)?;
        if let Some(key_registry) = deployments
            .deployment(ChainRole::Destination, "relayInfra.keyRegistry")
            .and_then(|addr| parse_address(&addr))
        {
            preflight_operator_keys(context, env_config, &dest_rpc, key_registry)?;
        }

        // Wait until at least one epoch has fully closed so that
        // `generate-genesis -e (currentEpoch - 1)` has a real captured snapshot
        // to work with. Without this gate, relay_utils errors for ~10 min with
        // a misleading "no contract code at given address" until the chain
        // catches up.
        wait_for_first_closed_epoch(&driver_address, &dest_rpc)?;

        // Always pass -e explicitly. The relay_utils default (currentEpoch - 1)
        // has an arithmetic overflow bug when currentEpoch is 0; the wait above
        // guarantees currentEpoch >= 1 so the subtraction is safe regardless.
        let driver = parse_address(&driver_address)
            .ok_or_else(|| eyre!("invalid driver address: {driver_address}"))?;
        let current_epoch = AlloyEth.current_epoch(&dest_rpc, driver).unwrap_or(0);
        let genesis_epoch = Some(current_epoch.saturating_sub(1));
        let secret_keys = format!("{}:{}", env_config.chains.destination.chain_id, genesis_key);
        let chains_arg = dest_rpc.clone();

        let preview_args = genesis_command_args(
            &chains_arg,
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            None,
            &secret_keys,
            genesis_epoch,
            false,
        );
        wait_for_genesis_ready(&preview_args)?;

        let commit_args = genesis_command_args(
            &chains_arg,
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            None,
            &secret_keys,
            genesis_epoch,
            true,
        );
        commit_genesis_with_chain_verification(
            &commit_args,
            &settlement_address,
            &dest_rpc,
            genesis_epoch.unwrap_or(0),
        )?;
    }

    verify_genesis(&settlement_address, &dest_rpc)?;
    Ok(())
}

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let relay = relay_addresses_from_deployments(context)?;

    ui::header(
        "refresh-genesis",
        &context.env_name,
        Some(env_config.active_provider.as_str()),
    );

    let step = ui::step("commit settlement genesis");
    ensure_genesis_for_relay(context, &env_config, &relay, true, env_config.is_local())?;
    step.done("settlement genesis committed");

    ui::ok("settlement genesis refreshed");
    ui::next(&format!("make validate ENV={}", context.env_name));
    Ok(())
}

pub fn ensure_local_epoch_fresh<E: EthApi>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    _eth: &E,
) -> Result<()> {
    if !env_config.is_local() {
        return Ok(());
    }

    let relay = relay_addresses_from_deployments(context)?;

    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    let dest_rpc = runtime
        .dest_rpc
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;
    let freshness_buffer = runtime::setting(context, "FRESHNESS_BUFFER_SECONDS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300);
    if genesis_is_stale(context, &relay.settlement, &dest_rpc, freshness_buffer)? {
        ui::warn("committed settlement epoch is stale; refreshing");
        ensure_genesis_for_relay(context, env_config, &relay, true, true)?;
    }

    Ok(())
}

fn relay_addresses_from_deployments(context: &ResolvedContext) -> Result<RelayInfraAddresses> {
    let deployments = DeploymentsConfig::load(&context.deployments)?;
    Ok(RelayInfraAddresses {
        driver: deployments
            .deployment(ChainRole::Destination, "relayInfra.driver")
            .map(|value| value.to_ascii_lowercase())
            .ok_or_else(|| {
                eyre!(
                    "relayInfra.driver not found in {}",
                    context.deployments.display()
                )
            })?,
        settlement: deployments
            .deployment(ChainRole::Destination, "relayInfra.settlement")
            .ok_or_else(|| {
                eyre!(
                    "relayInfra.settlement not found in {}",
                    context.deployments.display()
                )
            })?,
    })
}

fn genesis_exists(settlement_address: &str, dest_rpc: &str) -> Result<bool> {
    let Some(settlement_address) = parse_address(settlement_address) else {
        return Ok(false);
    };
    let Ok(last_epoch) = AlloyEth.last_committed_header_epoch(dest_rpc, settlement_address) else {
        return Ok(false);
    };
    Ok(AlloyEth
        .capture_timestamp(dest_rpc, settlement_address, last_epoch)
        .unwrap_or(0)
        != 0)
}

fn genesis_is_stale(
    context: &ResolvedContext,
    settlement_address: &str,
    dest_rpc: &str,
    freshness_buffer: u64,
) -> Result<bool> {
    let Some(settlement_address) = parse_address(settlement_address) else {
        return Ok(true);
    };
    let last_epoch = AlloyEth
        .last_committed_header_epoch(dest_rpc, settlement_address)
        .unwrap_or(0);

    let capture = AlloyEth
        .capture_timestamp(dest_rpc, settlement_address, last_epoch)
        .unwrap_or(0);
    if capture == 0 {
        return Ok(true);
    }

    let max_age = runtime::setting(context, "MAX_EPOCH_VALIDITY_SECONDS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7200);
    let freshness_threshold = max_age.saturating_sub(freshness_buffer);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    Ok(now.saturating_sub(capture) >= freshness_threshold)
}

fn fund_relay_keys(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    dest_rpc: &str,
    private_key: &str,
) -> Result<()> {
    let fund_amount = env_config.funding.operator_amount_wei.as_str();

    for signer in env_config.operator_signers(&context.project_root, &context.env_name)? {
        let _ = run_status(
            "cast",
            &[
                "send",
                &signer.address.to_string(),
                "--value",
                fund_amount,
                "--rpc-url",
                dest_rpc,
                "--private-key",
                private_key,
            ],
        );
    }
    Ok(())
}

/// Verify all operator BLS keys are registered on-chain before attempting genesis.
/// Retries a few times to handle transient RPC lag after deployment.
fn preflight_operator_keys(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    dest_rpc: &str,
    key_registry: Address,
) -> Result<()> {
    let mut step = ui::step("verify operator BLS keys");
    let operators = env_config.operator_signers(&context.project_root, &context.env_name)?;

    for attempt in 0..PREFLIGHT_RETRIES {
        let mut missing: Vec<String> = Vec::new();

        for (index, operator) in operators.iter().enumerate() {
            let operator_number = index + 1;
            let operator_address = operator.address;

            for tag in REQUIRED_KEY_TAGS {
                let key = AlloyEth
                    .key_bytes(dest_rpc, key_registry, operator_address, tag)
                    .unwrap_or_default();
                if key.is_empty() || key.iter().all(|b| *b == 0) {
                    missing.push(format!(
                        "operator {operator_number} ({operator_address}) missing BLS key tag {tag}"
                    ));
                }
            }
        }

        if missing.is_empty() {
            step.done("all operator BLS keys verified");
            return Ok(());
        }

        if attempt + 1 < PREFLIGHT_RETRIES {
            step.heartbeat_with(&format!(
                "{}; retrying in {}s (attempt {}/{})",
                missing[0],
                PREFLIGHT_RETRY_DELAY.as_secs(),
                attempt + 1,
                PREFLIGHT_RETRIES,
            ));
            thread::sleep(PREFLIGHT_RETRY_DELAY);
        } else {
            bail!(
                "operator BLS key pre-flight failed after {} attempts:\n  - {}",
                PREFLIGHT_RETRIES,
                missing.join("\n  - ")
            );
        }
    }

    unreachable!()
}

/// Wait for a deployed contract to be indexed by the RPC node.
/// Prevents "no contract code at given address" from transient RPC lag after deployment.
fn wait_for_contract(dest_rpc: &str, address: &str, label: &str) -> Result<()> {
    let address =
        parse_address(address).ok_or_else(|| eyre!("invalid {label} address: {address}"))?;

    for attempt in 0..30 {
        if AlloyEth.has_code(dest_rpc, address).unwrap_or(false) {
            if attempt > 0 {
                ui::ok(&format!("{label} contract indexed (waited {attempt}s)"));
            }
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("{label} contract at {address} has no code after 30s — deployment may have failed");
}

/// Wait until the Driver has a *closed* past epoch (`getCurrentEpoch() >= 1`).
///
/// `relay_utils generate-genesis -e N` reads `getEpochStart(N)` and treats N as
/// a closed snapshot. When N == 0 and the chain is still inside epoch 0,
/// relay_utils errors with a misleading "no contract code at given address"
/// for ~one full epoch. Calling it before this precondition wastes time and
/// confuses the operator. Block here instead.
fn wait_for_first_closed_epoch(driver_address: &str, dest_rpc: &str) -> Result<()> {
    let driver = parse_address(driver_address)
        .ok_or_else(|| eyre!("invalid driver address: {driver_address}"))?;

    if AlloyEth.current_epoch(dest_rpc, driver).unwrap_or(0) >= 1 {
        ui::ok("first epoch already closed");
        return Ok(());
    }

    // Compute initial ETA from epoch_start(1) so the heartbeat is informative.
    let epoch1_start = AlloyEth.epoch_start(dest_rpc, driver, 1).unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let initial_eta = epoch1_start.saturating_sub(now);

    let mut step = ui::step(format!(
        "wait for first epoch to close (~{}m{}s)",
        initial_eta / 60,
        initial_eta % 60
    ));

    let poll = Duration::from_secs(10);
    let timeout = Duration::from_secs(initial_eta.saturating_add(120).max(120));
    let deadline = std::time::Instant::now() + timeout;

    loop {
        let current = AlloyEth.current_epoch(dest_rpc, driver).unwrap_or(0);
        if current >= 1 {
            step.done(&format!("first epoch closed (currentEpoch={current})"));
            return Ok(());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let eta = epoch1_start.saturating_sub(now);
        step.heartbeat_with(&format!(
            "currentEpoch={current} (epoch boundary in ~{}m{}s)",
            eta / 60,
            eta % 60
        ));

        if std::time::Instant::now() >= deadline {
            bail!("timeout waiting for first epoch to close on driver {driver_address}");
        }
        thread::sleep(poll);
    }
}

fn local_bridge_network() -> Result<String> {
    let output = command_output(
        "docker",
        &[
            "network",
            "ls",
            "--filter",
            "name=bridge-network",
            "--format",
            "{{.Name}}",
        ],
    )?;
    output
        .lines()
        .find(|line| line.ends_with("_bridge-network"))
        .or_else(|| output.lines().find(|line| line.contains("bridge-network")))
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            eyre!("could not find bridge-network. Make sure Docker Compose services are running")
        })
}

#[allow(clippy::too_many_arguments)]
fn genesis_command_args(
    chains_arg: &str,
    driver_chain_id: u64,
    driver_address: &str,
    relay_image: &str,
    network_name: Option<String>,
    secret_keys: &str,
    epoch: Option<u64>,
    commit: bool,
) -> Vec<String> {
    let mut args = vec!["run".to_string(), "--rm".to_string()];
    if let Some(network_name) = network_name {
        args.push("--network".to_string());
        args.push(network_name);
    }
    args.extend([
        relay_image.to_string(),
        "/app/relay_utils".to_string(),
        "network".to_string(),
        "--chains".to_string(),
        chains_arg.to_string(),
        "--driver.address".to_string(),
        driver_address.to_string(),
        "--driver.chainid".to_string(),
        driver_chain_id.to_string(),
        "generate-genesis".to_string(),
    ]);
    if let Some(epoch) = epoch {
        args.push("-e".to_string());
        args.push(epoch.to_string());
    }
    if commit {
        args.push("--commit".to_string());
    } else {
        args.push("--json".to_string());
    }
    args.push("--secret-keys".to_string());
    args.push(secret_keys.to_string());
    args
}

fn wait_for_genesis_ready(args: &[String]) -> Result<()> {
    let mut waited = 0u64;
    let mut step = ui::step("wait for genesis readiness");
    #[allow(unused_assignments)]
    let mut last_error: Option<String> = None;
    let mut last_vp: Option<u128> = None;
    let mut stable_polls: usize = 0;

    loop {
        match preview_genesis(args) {
            Ok(preview) => {
                last_error = None;
                let vp = preview.header.total_voting_power;
                let quorum = preview.header.quorum_threshold;
                let quorum_met = vp >= quorum && vp > 0;

                if Some(vp) == last_vp {
                    stable_polls += 1;
                } else {
                    stable_polls = 1;
                }
                last_vp = Some(vp);

                if quorum_met && stable_polls >= GENESIS_STABLE_POLLS {
                    step.done(&format!(
                        "genesis ready (totalVP {vp}, stable for {stable_polls} polls)"
                    ));
                    return Ok(());
                }

                let reason = if quorum_met {
                    format!(
                        "totalVP={vp} (quorum met, stable {stable_polls}/{GENESIS_STABLE_POLLS} polls — waiting for late activations)"
                    )
                } else if vp == 0 {
                    "totalVP=0 (no operator stake in snapshot yet — vault deposits activate at next epoch boundary)".to_string()
                } else {
                    format!("totalVP={vp} (some operators activated, waiting for the rest)")
                };
                step.heartbeat_with(&reason);
            }
            Err(err) => {
                let raw = err.to_string();
                let short = extract_relay_error(&raw);

                // Fail fast on errors that won't resolve by waiting
                if is_permanent_error(&short) {
                    bail!("genesis generation failed: {short}");
                }

                // Heartbeat shows the short version; timeout shows full
                last_error = Some(raw);
                stable_polls = 0;
                last_vp = None;
                step.heartbeat_with(&format!("relay_utils: {short}"));
            }
        };

        if waited >= GENESIS_READY_TIMEOUT_SECONDS {
            if let Some(err) = last_error {
                bail!("timeout after {waited}s waiting for genesis readiness.\nLast error:\n{err}");
            }
            bail!("timeout after {waited}s waiting for genesis readiness");
        }

        thread::sleep(Duration::from_secs(GENESIS_READY_POLL_SECONDS));
        waited += GENESIS_READY_POLL_SECONDS;
    }
}

/// Extract the meaningful error line from relay_utils output, discarding the
/// full docker command, CLI help text, and stderr/stdout noise.
fn extract_relay_error(raw: &str) -> String {
    // Search for "Error: ..." anywhere in the output (may be mid-string, not at line start)
    if let Some(pos) = raw.find("Error: ") {
        let after = &raw[pos + 7..];
        // Take until end of line or end of string
        let end = after.find('\n').unwrap_or(after.len());
        let msg = after[..end].trim();
        if !msg.is_empty() {
            return msg.to_string();
        }
    }
    // Fall back: truncate the raw error
    let trimmed = raw.trim();
    if trimmed.len() > 120 {
        format!("{}...", &trimmed[..120])
    } else {
        trimmed.to_string()
    }
}

/// Errors that indicate misconfiguration — retrying won't help.
fn is_permanent_error(message: &str) -> bool {
    let permanent_patterns = [
        "failed to find key",
        "invalid key",
        "unknown network",
        "invalid address",
        "invalid chain",
    ];
    let lower = message.to_lowercase();
    permanent_patterns.iter().any(|p| lower.contains(p))
}

/// Commits genesis, then confirms it landed on-chain. The relay CLI's short
/// (~30s) `WaitForMined` timeout can fire before a Sepolia tx mines — exiting
/// `context deadline exceeded` even though the commit succeeds — so accept that
/// specific error iff the chain confirms the commit, and propagate any other.
fn commit_genesis_with_chain_verification(
    commit_args: &[String],
    settlement_address: &str,
    dest_rpc: &str,
    expected_epoch: u64,
) -> Result<()> {
    let exec_err = match run_status("docker", commit_args) {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };

    let raw = exec_err.to_string();
    if !raw.contains("context deadline exceeded") || !raw.contains("wait for tx mining") {
        return Err(exec_err);
    }

    // relay_utils gave up early. Confirm on-chain by polling Settlement until
    // the genesis header materializes, then proceed.
    let settlement = parse_address(settlement_address)
        .ok_or_else(|| eyre!("invalid settlement address: {settlement_address}"))?;
    let mut step = ui::step("verify genesis tx landed on-chain");
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let poll = Duration::from_secs(5);
    loop {
        if AlloyEth
            .capture_timestamp(dest_rpc, settlement, expected_epoch)
            .unwrap_or(0)
            != 0
        {
            step.done("genesis confirmed via on-chain state (relay_utils early-timeout was a false alarm)");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            step.done("genesis not yet visible on-chain");
            bail!(
                "relay_utils reported tx-wait timeout AND no committed header at settlement {settlement_address} after 120s; original error: {raw}"
            );
        }
        step.heartbeat_with("awaiting on-chain confirmation");
        thread::sleep(poll);
    }
}

fn preview_genesis(args: &[String]) -> Result<GenesisPreview> {
    let output = command_output("docker", args)?;
    let json_start = output
        .find('{')
        .ok_or_else(|| eyre!("genesis preview did not return json"))?;
    let json = &output[json_start..];
    serde_json::from_str(json).map_err(|err| eyre!("failed to parse genesis preview json: {err}"))
}

fn verify_genesis(settlement_address: &str, dest_rpc: &str) -> Result<()> {
    let settlement_address = parse_address(settlement_address)
        .ok_or_else(|| eyre!("invalid settlement address: {settlement_address}"))?;
    let _ = AlloyEth.last_committed_header_epoch(dest_rpc, settlement_address)?;
    Ok(())
}

fn run_status(program: &str, args: &[impl AsRef<std::ffi::OsStr>]) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    let args_display: Vec<_> = args.iter().map(|a| a.as_ref().to_string_lossy()).collect();
    let output = ui::run_command(&mut command, &format!("still running {program}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(ui::command_failure(
            &format!("{program} {}", args_display.join(" ")),
            &output
        )))
    }
}

fn command_output(program: &str, args: &[impl AsRef<std::ffi::OsStr>]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let args_display: Vec<_> = args.iter().map(|a| a.as_ref().to_string_lossy()).collect();
        Err(eyre!(ui::command_failure(
            &format!("{program} {}", args_display.join(" ")),
            &output
        )))
    }
}
