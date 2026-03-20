use std::process::Command;
use std::thread;
use std::time::Duration;

use eyre::{Result, bail, eyre};
use serde::Deserialize;

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::runtime;

const EPOCH_WAIT_TIMEOUT_SECONDS: u64 = 900;
const EPOCH_WAIT_POLL_SECONDS: u64 = 10;
const GENESIS_READY_TIMEOUT_SECONDS: u64 = 300;
const GENESIS_READY_POLL_SECONDS: u64 = 5;

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

pub fn ensure_genesis(context: &ResolvedContext, force: bool) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = DeploymentsConfig::load(&context.deployments)?;

    let relay = RelayInfraAddresses {
        driver: deployments
            .deployment(ChainRole::Destination, "relayInfra.driver")
            .map(|value| value.to_ascii_lowercase())
            .ok_or_else(|| eyre!("relayInfra.driver not found in {}", context.deployments.display()))?,
        settlement: deployments
            .deployment(ChainRole::Destination, "relayInfra.settlement")
            .ok_or_else(|| eyre!("relayInfra.settlement not found in {}", context.deployments.display()))?,
    };

    ensure_genesis_for_relay(context, &env_config, &relay, force, true)
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
        verify_genesis(&settlement_address, &dest_rpc)?;
        return Ok(());
    }

    if fund_keys {
        fund_relay_keys(context, &dest_rpc, &private_key, env_config.is_local())?;
    }

    let genesis_key =
        runtime::setting(context, "GENESIS_PRIVATE_KEY").unwrap_or_else(|| private_key.clone());
    let relay_image = runtime::setting(context, "RELAY_IMAGE")
        .unwrap_or_else(|| "symbioticfi/relay:1.0.1-20260305162153-f333c1a4e45c".to_string());

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
        run_status_owned("docker", &commit_args)?;
    } else {
        let current_epoch = wait_for_current_epoch(&driver_address, &dest_rpc)?;
        let genesis_epoch = current_epoch.saturating_sub(1);
        let secret_keys = format!("{}:{}", env_config.chains.destination.chain_id, genesis_key);

        let preview_args = genesis_command_args(
            &dest_rpc,
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            None,
            &secret_keys,
            Some(genesis_epoch),
            false,
        );
        wait_for_genesis_ready(&preview_args)?;

        let commit_args = genesis_command_args(
            &dest_rpc,
            env_config.chains.destination.chain_id,
            &driver_address,
            &relay_image,
            None,
            &secret_keys,
            Some(genesis_epoch),
            true,
        );
        run_status_owned("docker", &commit_args)?;
    }

    verify_genesis(&settlement_address, &dest_rpc)?;
    Ok(())
}

fn genesis_exists(settlement_address: &str, dest_rpc: &str) -> Result<bool> {
    let Some(settlement_address) = parse_address(settlement_address) else {
        return Ok(false);
    };
    Ok(AlloyEth
        .last_committed_header_epoch(dest_rpc, settlement_address)
        .unwrap_or(0)
        != 0)
}

fn fund_relay_keys(
    context: &ResolvedContext,
    dest_rpc: &str,
    private_key: &str,
    is_local: bool,
) -> Result<()> {
    let deployer_key =
        runtime::setting(context, "DEPLOYER_PRIVATE_KEY").unwrap_or_else(|| private_key.to_string());
    let fund_amount = if is_local {
        "1ether".to_string()
    } else {
        runtime::setting(context, "OPERATOR_FUND_AMOUNT").unwrap_or_else(|| "0.2ether".to_string())
    };

    for index in 0..3 {
        let operator_private_key = runtime::operator_private_key(context, index)
            .ok_or_else(|| eyre!("OPERATOR_{}_PRIVATE_KEY is not set", index + 1))?;
        let operator_address = AlloyEth.address_from_private_key(&operator_private_key)?;
        let _ = run_status(
            "cast",
            &[
                "send",
                &operator_address.to_string(),
                "--value",
                &fund_amount,
                "--rpc-url",
                dest_rpc,
                "--private-key",
                &deployer_key,
            ],
        );
    }
    Ok(())
}

fn wait_for_current_epoch(driver_address: &str, dest_rpc: &str) -> Result<u64> {
    let mut waited = 0u64;
    let driver_address =
        parse_address(driver_address).ok_or_else(|| eyre!("invalid driver address: {driver_address}"))?;
    loop {
        let current_epoch = AlloyEth.current_epoch(dest_rpc, driver_address).unwrap_or(0);
        if current_epoch >= 1 {
            return Ok(current_epoch);
        }

        if waited >= EPOCH_WAIT_TIMEOUT_SECONDS {
            bail!("timeout waiting for Driver epoch >= 1 (15 min)");
        }

        thread::sleep(Duration::from_secs(EPOCH_WAIT_POLL_SECONDS));
        waited += EPOCH_WAIT_POLL_SECONDS;
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
        .ok_or_else(|| eyre!("could not find bridge-network. Make sure Docker Compose services are running"))
}

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

    loop {
        let last_reason = match preview_genesis(args) {
            Ok(preview) => {
                if preview.header.total_voting_power >= preview.header.quorum_threshold
                    && preview.header.total_voting_power > 0
                {
                    return Ok(());
                }
                format!(
                    "genesis not ready: totalVotingPower {} < quorumThreshold {}",
                    preview.header.total_voting_power, preview.header.quorum_threshold
                )
            }
            Err(err) => {
                err.to_string()
            }
        };

        if waited >= GENESIS_READY_TIMEOUT_SECONDS {
            bail!(
                "timeout waiting for genesis readiness: {}",
                last_reason
            );
        }

        thread::sleep(Duration::from_secs(GENESIS_READY_POLL_SECONDS));
        waited += GENESIS_READY_POLL_SECONDS;
    }
}

fn preview_genesis(args: &[String]) -> Result<GenesisPreview> {
    let output = command_output_owned("docker", args)?;
    let json_start = output
        .find('{')
        .ok_or_else(|| eyre!("genesis preview did not return json"))?;
    let json = &output[json_start..];
    serde_json::from_str(json).map_err(|err| eyre!("failed to parse genesis preview json: {err}"))
}

fn verify_genesis(settlement_address: &str, dest_rpc: &str) -> Result<()> {
    let settlement_address =
        parse_address(settlement_address).ok_or_else(|| eyre!("invalid settlement address: {settlement_address}"))?;
    let _ = AlloyEth.last_committed_header_epoch(dest_rpc, settlement_address)?;
    Ok(())
}

fn run_status(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("`{program} {}` failed with status {status}", args.join(" ")))
    }
}

fn run_status_owned(program: &str, args: &[String]) -> Result<()> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("`{program} {}` failed with status {status}", args.join(" ")))
    }
}

fn command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(eyre!(
            "`{program} {}` failed with status {}",
            args.join(" "),
            output.status
        ))
    }
}

fn command_output_owned(program: &str, args: &[String]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(eyre!(
            "`{program} {}` failed with status {}",
            args.join(" "),
            output.status
        ))
    }
}
