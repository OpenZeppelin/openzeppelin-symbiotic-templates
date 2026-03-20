use std::fs;
use std::process::Command;

use eyre::{Result, bail, eyre};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::bridge;
use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::genesis;
use crate::preflight;
use crate::publish;
use crate::render;
use crate::runner::{CommandRunner, SystemRunner};
use crate::runtime;
use crate::validate;

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let runner = SystemRunner;
    let eth = AlloyEth;

    match deploy_mode(context, &env_config)? {
        DeployMode::Bridge => bridge::run_make_target(context, "deploy"),
        DeployMode::ReconcileExisting => reconcile_existing(context, &env_config, &runner, &eth),
        DeployMode::FirstRunLocalLayerzero => {
            first_run_local_layerzero(context, &env_config, &runner, &eth)
        }
        DeployMode::FirstRunNonLocalLayerzero => {
            first_run_non_local_layerzero(context, &env_config, &runner, &eth)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeployMode {
    Bridge,
    ReconcileExisting,
    FirstRunLocalLayerzero,
    FirstRunNonLocalLayerzero,
}

struct DeployInputs {
    source_rpc: String,
    dest_rpc: String,
    private_key: String,
}

fn deploy_mode(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<DeployMode> {
    if runtime::setting(context, "FORCE_RELAY_DEPLOY").as_deref() == Some("1") {
        return Ok(DeployMode::Bridge);
    }

    let deployments = match DeploymentsConfig::load(&context.deployments) {
        Ok(deployments) => deployments,
        Err(_) => {
            return if env_config.active_provider == "layerzero" && env_config.is_local() {
                Ok(DeployMode::FirstRunLocalLayerzero)
            } else if !env_config.is_local() && env_config.active_provider == "layerzero" {
                Ok(DeployMode::FirstRunNonLocalLayerzero)
            } else {
                Ok(DeployMode::Bridge)
            };
        }
    };

    if deployments.role_has_entries(ChainRole::Source)
        && deployments.role_has_entries(ChainRole::Destination)
    {
        Ok(DeployMode::ReconcileExisting)
    } else if env_config.active_provider == "layerzero" && env_config.is_local() {
        Ok(DeployMode::FirstRunLocalLayerzero)
    } else if !env_config.is_local() && env_config.active_provider == "layerzero" {
        Ok(DeployMode::FirstRunNonLocalLayerzero)
    } else {
        Ok(DeployMode::Bridge)
    }
}

fn reconcile_existing<R: CommandRunner>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    _runner: &R,
    eth: &AlloyEth,
) -> Result<()> {
    println!(
        "═══ Deploy artifacts already exist for {}, reconciling generated state... ═══",
        env_config.active_provider
    );

    render::render(context)?;
    maybe_configure_ccv(context, env_config)?;
    maybe_refresh_genesis(context, env_config, eth)?;
    ensure_runtime_valid(context, env_config.is_local(), eth)?;

    println!("Deployment state is valid.");
    Ok(())
}

fn first_run_non_local_layerzero<R: CommandRunner>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    runner: &R,
    eth: &AlloyEth,
) -> Result<()> {
    first_run_layerzero(context, env_config, runner, eth, false)
}

fn first_run_local_layerzero<R: CommandRunner>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    runner: &R,
    eth: &AlloyEth,
) -> Result<()> {
    first_run_layerzero(context, env_config, runner, eth, true)
}

fn maybe_configure_ccv(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    if env_config.active_provider == "chainlink_ccv" {
        bridge::run_make_target(context, "configure-ccv-contracts")?;
    }
    Ok(())
}

fn maybe_refresh_genesis<E: EthApi>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    eth: &E,
) -> Result<()> {
    if env_config.is_local() || genesis_refresh_needed(context, eth)?.unwrap_or(false) == false {
        return Ok(());
    }

    println!("Refreshing settlement genesis before validation...");
    genesis::ensure_genesis(context, true)
}

fn first_run_layerzero<R: CommandRunner>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    _runner: &R,
    eth: &AlloyEth,
    local: bool,
) -> Result<()> {
    let inputs = deploy_inputs(context, env_config)?;

    if local {
        println!("═══ First deploy for layerzero: local devnet ═══");
    } else {
        println!("═══ First deploy for layerzero: non-local shared network ═══");
    }

    println!("[1/5] Building contracts...");
    run_contracts_command(context, "forge", &["build", "--quiet"], &[])?;

    if local {
        println!("[2/5] Building operator image and starting local chains...");
        prepare_local_first_run(context)?;
        println!("[3/5] Waiting for local RPCs...");
        wait_for_rpc(eth, &inputs.source_rpc, "anvil")?;
        wait_for_rpc(eth, &inputs.dest_rpc, "anvil-settlement")?;
    } else {
        println!("[2/5] Verifying external RPC connectivity...");
        wait_for_rpc(eth, &inputs.source_rpc, "source chain")?;
        wait_for_rpc(eth, &inputs.dest_rpc, "destination chain")?;
    }

    println!("[{}] Deploying layerzero contracts...", if local { "4/5" } else { "3/5" });
    deploy_layerzero(context, env_config, &inputs, local)?;

    println!("[4/5] Rendering generated config...");
    render::render(context)?;

    println!("[5/5] Validating deployment...");
    ensure_runtime_valid(context, local, eth)?;

    if local {
        println!("Deployment complete. Run `make start` to start services.");
    } else {
        println!("Deployment complete. Run `make validate ENV={}` or start services.", context.env_name);
    }
    Ok(())
}

fn deploy_layerzero(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    inputs: &DeployInputs,
    local: bool,
) -> Result<()> {
    let deploy_data = contracts_deploy_data_dir(context);
    fs::create_dir_all(&deploy_data)?;

    if !local {
        write_layerzero_endpoint_files(context, env_config)?;
    }
    if !local && env_config.relay.epoch_start_delay_seconds == 0 {
        bail!(
            "relay.epochStartDelaySeconds must be > 0 for external networks (timestamp drift causes revert)"
        );
    }

    let relay_env = relay_deploy_envs(context, env_config)?;
    deploy_relay_infra_with_retries(context, &inputs.dest_rpc, &inputs.private_key, &relay_env)?;
    let relay = refresh_relay_infra_deploy_data_from_broadcast(context, env_config)?;

    run_layerzero_stack(
        context,
        local,
        &layerzero_stack_envs(
            context,
            env_config,
            &inputs.source_rpc,
            &inputs.dest_rpc,
            &inputs.private_key,
        )?,
    )?;

    publish::publish(context)?;
    if local {
        mine_block(&inputs.source_rpc)?;
        mine_block(&inputs.dest_rpc)?;
    }
    genesis::ensure_genesis_for_relay(
        context,
        env_config,
        &relay,
        false,
        local,
    )?;
    Ok(())
}

fn wait_for_rpc<E: EthApi>(eth: &E, rpc_url: &str, name: &str) -> Result<()> {
    for _ in 0..30 {
        if eth.rpc_reachable(rpc_url) {
            println!("      ✓ {name} ready");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    bail!("timeout waiting for {name} ({rpc_url})");
}

fn ensure_runtime_valid<E: EthApi>(
    context: &ResolvedContext,
    managed_operators: bool,
    eth: &E,
) -> Result<()> {
    let preflight = preflight::preflight(context, eth);
    if !preflight.failures.is_empty() {
        eprintln!("Preflight checks failed:");
        for failure in preflight.failures {
            eprintln!("  - {failure}");
        }
        bail!("startup preflight failed");
    }

    let validation = validate::validate(context, managed_operators, eth);
    if !validation.failures.is_empty() {
        eprintln!("Validation failed:");
        for failure in validation.failures {
            eprintln!("  - {failure}");
        }
        bail!("runtime validation failed");
    }

    Ok(())
}

fn refresh_relay_infra_deploy_data_from_broadcast(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<genesis::RelayInfraAddresses> {
    let broadcast_path = context
        .project_root
        .join("contracts")
        .join("broadcast")
        .join("DeployRelayInfra.s.sol")
        .join(env_config.chains.destination.chain_id.to_string())
        .join("run-latest.json");
    let body = fs::read_to_string(&broadcast_path)
        .map_err(|err| eyre!("failed to read {}: {err}", broadcast_path.display()))?;
    let broadcast: Value = serde_json::from_str(&body)
        .map_err(|err| eyre!("failed to parse {}: {err}", broadcast_path.display()))?;

    let relay_infra = json!({
        "chainId": env_config.chains.destination.chain_id,
        "network": broadcast_created_address(&broadcast, "Network")?,
        "keyRegistry": broadcast_created_address(&broadcast, "KeyRegistry")?,
        "votingPowers": broadcast_created_address(&broadcast, "VotingPowers")?,
        "settlement": broadcast_created_address(&broadcast, "Settlement")?,
        "driver": broadcast_created_address(&broadcast, "Driver")?,
        "stakingToken": broadcast_created_address(&broadcast, "MockERC20")?,
        "vaultFactory": broadcast_created_address(&broadcast, "VaultFactory")?,
        "operatorRegistry": broadcast_created_address(&broadcast, "OperatorRegistry")?,
        "networkRegistry": broadcast_created_address(&broadcast, "NetworkRegistry")?,
    });

    let relay_infra_path = contracts_deploy_data_dir(context).join("relay_infra.json");
    fs::write(
        &relay_infra_path,
        format!("{}\n", serde_json::to_string_pretty(&relay_infra)?),
    )
    .map_err(|err| eyre!("failed to write {}: {err}", relay_infra_path.display()))?;

    Ok(genesis::RelayInfraAddresses {
        driver: relay_infra["driver"]
            .as_str()
            .ok_or_else(|| eyre!("missing driver in {}", relay_infra_path.display()))?
            .to_string(),
        settlement: relay_infra["settlement"]
            .as_str()
            .ok_or_else(|| eyre!("missing settlement in {}", relay_infra_path.display()))?
            .to_string(),
    })
}

fn broadcast_created_address(broadcast: &Value, contract_name: &str) -> Result<String> {
    broadcast
        .get("transactions")
        .and_then(Value::as_array)
        .and_then(|transactions| {
            transactions.iter().find_map(|tx| {
                let name = tx.get("contractName").and_then(Value::as_str)?;
                let tx_type = tx.get("transactionType").and_then(Value::as_str)?;
                let address = tx.get("contractAddress").and_then(Value::as_str)?;
                (name == contract_name && tx_type == "CREATE").then(|| address.to_string())
            })
        })
        .ok_or_else(|| eyre!("missing CREATE address for {contract_name} in relay broadcast"))
}

fn write_layerzero_endpoint_files(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    let deploy_data = contracts_deploy_data_dir(context);
    let source = json!({
        "chainId": env_config.chains.source.chain_id,
        "eid": env_config.chains.source.eid,
        "endpoint": env_config.predeploy(ChainRole::Source, "layerzero", "endpoint")
            .ok_or_else(|| eyre!("missing source layerzero endpoint predeploy"))?,
        "sendUln": env_config.predeploy(ChainRole::Source, "layerzero", "sendUln302")
            .ok_or_else(|| eyre!("missing source layerzero sendUln302 predeploy"))?,
    });
    fs::write(
        deploy_data.join("layerzero_source.json"),
        format!("{}\n", serde_json::to_string_pretty(&source)?),
    )?;

    let dest = json!({
        "chainId": env_config.chains.destination.chain_id,
        "eid": env_config.chains.destination.eid,
        "endpoint": env_config.predeploy(ChainRole::Destination, "layerzero", "endpoint")
            .ok_or_else(|| eyre!("missing destination layerzero endpoint predeploy"))?,
        "receiveUln": env_config.predeploy(ChainRole::Destination, "layerzero", "receiveUln302")
            .ok_or_else(|| eyre!("missing destination layerzero receiveUln302 predeploy"))?,
    });
    fs::write(
        deploy_data.join("layerzero_dest.json"),
        format!("{}\n", serde_json::to_string_pretty(&dest)?),
    )?;
    Ok(())
}

fn layerzero_stack_envs(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    source_rpc: &str,
    dest_rpc: &str,
    private_key: &str,
) -> Result<Vec<(String, String)>> {
    let mut envs = vec![
        ("SOURCE_RPC_URL".to_string(), source_rpc.to_string()),
        ("DEST_RPC_URL".to_string(), dest_rpc.to_string()),
        ("PRIVATE_KEY".to_string(), private_key.to_string()),
        (
            "LZ_SOURCE_CHAIN_ID".to_string(),
            env_config.chains.source.chain_id.to_string(),
        ),
        (
            "LZ_DEST_CHAIN_ID".to_string(),
            env_config.chains.destination.chain_id.to_string(),
        ),
        (
            "LZ_SOURCE_EID".to_string(),
            env_config.chains.source.eid.to_string(),
        ),
        (
            "LZ_DEST_EID".to_string(),
            env_config.chains.destination.eid.to_string(),
        ),
    ];

    for index in 0..3 {
        let key = runtime::operator_private_key(context, index)
            .ok_or_else(|| eyre!("OPERATOR_{}_PRIVATE_KEY is not set", index + 1))?;
        envs.push((format!("OPERATOR_{}_PRIVATE_KEY", index + 1), key));
    }

    Ok(envs)
}

fn relay_deploy_envs(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<Vec<(String, String)>> {
    let mut envs = relay_envs(env_config);

    if !env_config.is_local() {
        envs.push(symbiotic_core_config(context, env_config)?);
    }

    for index in 0..3 {
        let key = runtime::operator_private_key(context, index)
            .ok_or_else(|| eyre!("OPERATOR_{}_PRIVATE_KEY is not set", index + 1))?;
        envs.push((format!("OPERATOR_{}_PRIVATE_KEY", index + 1), key));

        let signer_key = format!("SIGNER_{}_ADDRESS", index + 1);
        if let Some(value) = runtime::setting(context, &signer_key).filter(|value| !value.is_empty()) {
            envs.push((signer_key, value));
        }
    }

    if let Some(value) = runtime::setting(context, "RELAYER_SIGNER_FUND_AMOUNT").filter(|value| !value.is_empty()) {
        envs.push(("RELAYER_SIGNER_FUND_AMOUNT".to_string(), value));
    }

    Ok(envs)
}

fn relay_envs(env_config: &EnvironmentConfig) -> Vec<(String, String)> {
    vec![
        (
            "EPOCH_DURATION".to_string(),
            env_config.relay.epoch_duration_seconds.to_string(),
        ),
        (
            "SLASHING_WINDOW".to_string(),
            env_config.relay.slashing_window_seconds.to_string(),
        ),
        (
            "EPOCH_START_DELAY".to_string(),
            env_config.relay.epoch_start_delay_seconds.to_string(),
        ),
    ]
}

fn symbiotic_core_config(
    _context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<(String, String)> {
    let temp = NamedTempFile::new()?;
    let body = json!({
        env_config.chains.destination.chain_id.to_string(): env_config.chains.destination.predeploys["symbioticCore"].clone()
    });
    fs::write(temp.path(), format!("{}\n", serde_json::to_string_pretty(&body)?))?;
    let (_file, path) = temp.keep()?;
    Ok((
        "SYMBIOTIC_CORE_CONFIG".to_string(),
        path.display().to_string(),
    ))
}

fn run_layerzero_stack(context: &ResolvedContext, local: bool, envs: &[(String, String)]) -> Result<()> {
    let mut args = vec![
        "script",
        "script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack",
        "--sig",
        if local { "deployLocal()" } else { "deployExternal()" },
        "--broadcast",
        "--multi",
        "--non-interactive",
        "--private-key",
    ];
    let private_key = envs
        .iter()
        .find(|(key, _)| key == "PRIVATE_KEY")
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?;
    args.push(private_key);
    if !local {
        args.push("--slow");
    }
    args.push("--quiet");

    run_contracts_command(context, "forge", &args, envs)
}

fn deploy_inputs(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<DeployInputs> {
    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    Ok(DeployInputs {
        source_rpc: runtime
            .source_rpc
            .ok_or_else(|| eyre!("SOURCE RPC is not configured"))?,
        dest_rpc: runtime
            .dest_rpc
            .ok_or_else(|| eyre!("DEST RPC is not configured"))?,
        private_key: runtime
            .private_key
            .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?,
    })
}

fn prepare_local_first_run(context: &ResolvedContext) -> Result<()> {
    run_project_command(
        context,
        "docker",
        &[
            "compose",
            "-f",
            "docker-compose.yml",
            "-f",
            "docker-compose.local.yml",
            "--profile",
            "dev",
            "build",
            "--quiet",
            "operator-1",
        ],
        &[],
    )?;
    run_project_command(
        context,
        "docker",
        &[
            "compose",
            "-f",
            "docker-compose.yml",
            "-f",
            "docker-compose.local.yml",
            "--profile",
            "infra",
            "up",
            "-d",
            "--remove-orphans",
        ],
        &[("ENV".to_string(), context.env_name.clone())],
    )
}

fn mine_block(rpc_url: &str) -> Result<()> {
    AlloyEth.mine_block(rpc_url)
}

fn deploy_relay_infra_with_retries(
    context: &ResolvedContext,
    dest_rpc: &str,
    private_key: &str,
    envs: &[(String, String)],
) -> Result<()> {
    let timeout = runtime::setting(context, "FORGE_BROADCAST_TIMEOUT")
        .unwrap_or_else(|| "180".to_string());
    for attempt in 1..=3 {
        let mut args = vec![
            "script",
            "script/DeployRelayInfra.s.sol:DeployRelayInfra",
            "--rpc-url",
            dest_rpc,
            "--broadcast",
            "--private-key",
            private_key,
            "--code-size-limit",
            "50000",
            "--gas-estimate-multiplier",
            if attempt > 1 { "200" } else { "150" },
            "--timeout",
            &timeout,
            "--slow",
            "--non-interactive",
        ];
        if attempt > 1 {
            args.push("--resume");
        }
        if run_contracts_command(context, "forge", &args, envs).is_ok() {
            return Ok(());
        }
    }
    bail!("relay infra deployment failed after 3 attempts")
}

fn contracts_deploy_data_dir(context: &ResolvedContext) -> std::path::PathBuf {
    context
        .project_root
        .join("contracts")
        .join("deploy-data")
}

fn run_contracts_command(
    context: &ResolvedContext,
    program: &str,
    args: &[&str],
    envs: &[(String, String)],
) -> Result<()> {
    run_command_status(
        program,
        args,
        envs,
        Some(context.project_root.join("contracts")),
    )
}

fn run_command_status(
    program: &str,
    args: &[&str],
    envs: &[(String, String)],
    current_dir: Option<std::path::PathBuf>,
) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    command.envs(envs.iter().map(|(key, value)| (key, value)));
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("`{program} {}` failed with status {status}", args.join(" ")))
    }
}

fn run_project_command(
    context: &ResolvedContext,
    program: &str,
    args: &[&str],
    envs: &[(String, String)],
) -> Result<()> {
    run_command_status(program, args, envs, Some(context.project_root.clone()))
}

fn genesis_refresh_needed<E: EthApi>(
    context: &ResolvedContext,
    eth: &E,
) -> Result<Option<bool>> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = DeploymentsConfig::load(&context.deployments)?;
    let runtime = runtime::RuntimeInputs::resolve(context, &env_config);

    let Some(settlement) = deployments.deployment(ChainRole::Destination, "relayInfra.settlement") else {
        return Ok(None);
    };
    let Some(dest_rpc) = runtime.dest_rpc else {
        return Ok(None);
    };
    let Some(settlement) = parse_address(&settlement) else {
        return Ok(None);
    };
    let Ok(epoch) = eth.last_committed_header_epoch(&dest_rpc, settlement) else {
        return Ok(None);
    };
    if epoch == 0 {
        return Ok(None);
    }

    let Ok(capture) = eth.capture_timestamp(&dest_rpc, settlement, epoch) else {
        return Ok(None);
    };
    if capture == 0 {
        return Ok(None);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let max_age = runtime::setting(context, "MAX_EPOCH_VALIDITY_SECONDS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7200);

    Ok(Some(now.saturating_sub(capture) >= max_age))
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::context::ResolvedContext;

    fn write_context(env_body: &str, deployments_body: &str, env_name: &str) -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join(format!("{env_name}.json"));
        let deployments = root.join("deployments.json");
        fs::write(&env_config, env_body).unwrap();
        fs::write(&deployments, deployments_body).unwrap();
        std::mem::forget(temp_dir);

        ResolvedContext {
            project_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf(),
            env_name: env_name.to_string(),
            env_config,
            deployments,
            generated_dir: root.join("generated").join(env_name),
        }
    }

    use std::path::PathBuf;

    #[test]
    fn uses_native_first_run_for_missing_local_layerzero_deployments() {
        let context = write_context(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                }
            }"#,
            r#"{ "source": {}, "destination": {} }"#,
            "local",
        );
        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();

        assert_eq!(
            deploy_mode(&context, &env_config).unwrap(),
            DeployMode::FirstRunLocalLayerzero
        );
    }

    #[test]
    fn keeps_existing_local_deploy_native() {
        let context = write_context(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                }
            }"#,
            r#"{ "source": { "dvn": "0x1" }, "destination": { "dvn": "0x2" } }"#,
            "local",
        );
        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();

        assert_eq!(
            deploy_mode(&context, &env_config).unwrap(),
            DeployMode::ReconcileExisting
        );
    }

    #[test]
    fn uses_native_first_run_for_missing_non_local_layerzero_deployments() {
        let context = write_context(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                }
            }"#,
            r#"{ "source": {}, "destination": {} }"#,
            "testnet",
        );
        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();

        assert_eq!(
            deploy_mode(&context, &env_config).unwrap(),
            DeployMode::FirstRunNonLocalLayerzero
        );
    }

    #[test]
    fn keeps_existing_non_local_deploy_native() {
        let context = write_context(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                }
            }"#,
            r#"{ "source": { "dvn": "0x1" }, "destination": { "dvn": "0x2" } }"#,
            "testnet",
        );
        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();

        assert_eq!(
            deploy_mode(&context, &env_config).unwrap(),
            DeployMode::ReconcileExisting
        );
    }

    #[test]
    fn refreshes_relay_infra_deploy_data_from_broadcast() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join("local.json");
        let deployments = root.join("deployments.json");
        let broadcast_dir = root
            .join("contracts")
            .join("broadcast")
            .join("DeployRelayInfra.s.sol")
            .join("31338");
        let deploy_data_dir = root.join("contracts").join("deploy-data");
        fs::create_dir_all(&broadcast_dir).unwrap();
        fs::create_dir_all(&deploy_data_dir).unwrap();

        fs::write(
            &env_config,
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                }
            }"#,
        )
        .unwrap();
        fs::write(&deployments, r#"{ "source": {}, "destination": {} }"#).unwrap();
        fs::write(
            broadcast_dir.join("run-latest.json"),
            r#"{
                "transactions": [
                    { "contractName": "Network", "transactionType": "CREATE", "contractAddress": "0x1111111111111111111111111111111111111111" },
                    { "contractName": "KeyRegistry", "transactionType": "CREATE", "contractAddress": "0x2222222222222222222222222222222222222222" },
                    { "contractName": "VotingPowers", "transactionType": "CREATE", "contractAddress": "0x3333333333333333333333333333333333333333" },
                    { "contractName": "Settlement", "transactionType": "CREATE", "contractAddress": "0x4444444444444444444444444444444444444444" },
                    { "contractName": "Driver", "transactionType": "CREATE", "contractAddress": "0x5555555555555555555555555555555555555555" },
                    { "contractName": "MockERC20", "transactionType": "CREATE", "contractAddress": "0x6666666666666666666666666666666666666666" },
                    { "contractName": "VaultFactory", "transactionType": "CREATE", "contractAddress": "0x7777777777777777777777777777777777777777" },
                    { "contractName": "OperatorRegistry", "transactionType": "CREATE", "contractAddress": "0x8888888888888888888888888888888888888888" },
                    { "contractName": "NetworkRegistry", "transactionType": "CREATE", "contractAddress": "0x9999999999999999999999999999999999999999" }
                ]
            }"#,
        )
        .unwrap();
        std::mem::forget(temp_dir);

        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "local".to_string(),
            env_config,
            deployments,
            generated_dir: root.join("generated").join("local"),
        };
        let env = EnvironmentConfig::load(&context.env_config).unwrap();

        let relay = refresh_relay_infra_deploy_data_from_broadcast(&context, &env).unwrap();
        assert_eq!(relay.driver, "0x5555555555555555555555555555555555555555");
        assert_eq!(relay.settlement, "0x4444444444444444444444444444444444444444");

        let written = fs::read_to_string(deploy_data_dir.join("relay_infra.json")).unwrap();
        let json: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(json["driver"], "0x5555555555555555555555555555555555555555");
        assert_eq!(json["settlement"], "0x4444444444444444444444444444444444444444");
        assert_eq!(json["networkRegistry"], "0x9999999999999999999999999999999999999999");
    }
}
