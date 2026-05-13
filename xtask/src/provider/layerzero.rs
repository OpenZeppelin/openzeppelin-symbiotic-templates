use std::fs;
use std::path::Path;
use std::process::Command;

use eyre::{Result, bail, eyre};
use serde_json::{Value, json};

use crate::addresses;
use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{EthApi, parse_address};
use crate::generate::{read_json_value, write_pretty_json};
use crate::genesis;
use crate::publish;
use crate::runtime;
use crate::signers;
use crate::ui;

pub fn deploy(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    let source_rpc = runtime
        .source_rpc
        .ok_or_else(|| eyre!("SOURCE RPC is not configured"))?;
    let dest_rpc = runtime
        .dest_rpc
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;
    let private_key = runtime
        .private_key
        .ok_or_else(|| eyre!("PRIVATE_KEY is not configured"))?;

    let deploy_data = contracts_deploy_data_dir(context);
    fs::create_dir_all(&deploy_data)?;
    if !env_config.layerzero_oapp_enabled() {
        clear_example_oapp_deploy_data(context)?;
    }

    if !env_config.is_local() {
        write_layerzero_endpoint_files(context, env_config)?;
    }
    if !env_config.is_local() && env_config.relay.epoch_start_delay_seconds == 0 {
        bail!(
            "relay.epochStartDelaySeconds must be > 0 for external networks (timestamp drift causes revert)"
        );
    }

    let relay_env = relay_deploy_envs(context, env_config)?;
    let relay_step = ui::step("deploy relay infrastructure");
    deploy_relay_infra_with_retries(
        context,
        env_config.chains.destination.chain_id,
        &dest_rpc,
        &private_key,
        &relay_env,
    )?;
    relay_step.done("relay infrastructure deployed");

    let relay_data_step = ui::step("sync relay deployment state");
    let relay = refresh_relay_infra_deploy_data_from_broadcast(context, env_config)?;
    relay_data_step.done("relay deployment state synced");

    let stack_step = ui::step("deploy layerzero contracts");
    run_layerzero_stack(
        context,
        env_config.is_local(),
        &layerzero_stack_envs(context, env_config, &source_rpc, &dest_rpc, &private_key)?,
    )?;
    stack_step.done("layerzero contracts deployed");

    let publish_step = ui::step("checkpoint deployment state");
    checkpoint_deployment_state(context)?;
    publish_step.done("deployment state checkpointed");

    if env_config.is_local() {
        let blocks_step = ui::step("mine local blocks");
        mine_block(&source_rpc)?;
        mine_block(&dest_rpc)?;
        blocks_step.done("local blocks mined");
    }

    let genesis_step = ui::step("commit settlement genesis");
    genesis::ensure_genesis_for_relay(context, env_config, &relay, false, env_config.is_local())?;
    genesis_step.done("settlement genesis committed");
    Ok(())
}

pub fn validate_chain_state<E: EthApi>(
    deployments: &DeploymentsConfig,
    runtime: &runtime::RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let src_dvn = deployments.deployment(ChainRole::Source, "dvn");
    let dst_dvn = deployments.deployment(ChainRole::Destination, "dvn");
    let settlement = deployments.deployment(ChainRole::Destination, "relayInfra.settlement");

    check_code(
        runtime.source_rpc.as_deref(),
        src_dvn.as_deref(),
        "source DVN",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_dvn.as_deref(),
        "destination DVN",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        settlement.as_deref(),
        "relayInfra.settlement",
        eth,
        failures,
    );

    if let (Some(dest_rpc), Some(dst_dvn), Some(settlement)) = (
        runtime.dest_rpc.as_deref(),
        dst_dvn.as_deref(),
        settlement.as_deref(),
    ) && parse_address(dst_dvn).is_some()
        && parse_address(settlement).is_some()
    {
        let actual = parse_address(dst_dvn)
            .and_then(|address| eth.settlement_address(dest_rpc, address).ok())
            .map(|value| value.to_string());
        if let Some(actual) = actual
            && !actual.eq_ignore_ascii_case(settlement)
        {
            failures.push(format!(
                "destination DVN settlement mismatch: expected {settlement}, got {}",
                actual.to_ascii_lowercase()
            ));
        }
    }
}

pub fn validate_configuration(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    failures: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    require_deployment(
        deployments.deployment(ChainRole::Source, "dvn"),
        "missing source DVN deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "dvn"),
        "missing destination DVN deployment in deployments file",
        failures,
    );
    if !env_config.layerzero_oapp_enabled() {
        warnings.push(
            "LayerZero starter OApp is disabled in config (`layerzero.oapp.enabled: false`); `make send` and `make e2e` are unavailable."
                .to_string(),
        );
    }

    if !env_config.is_local() {
        for (key, label) in [
            ("vaultFactory", "destination symbiotic core vaultFactory"),
            (
                "operatorRegistry",
                "destination symbiotic core operatorRegistry",
            ),
            (
                "networkRegistry",
                "destination symbiotic core networkRegistry",
            ),
        ] {
            if let Err(err) = addresses::require(
                env_config,
                deployments,
                ChainRole::Destination,
                &format!("symbioticCore.{key}"),
                Some(("symbioticCore", key)),
                label,
            ) {
                failures.push(err.to_string());
            }
        }
    }
}

pub fn render_monitor_definition(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    templates_root: &Path,
    generated_dir: &Path,
) -> Result<()> {
    let address = deployments
        .deployment(ChainRole::Source, "dvn")
        .ok_or_else(|| eyre!("missing monitor address for provider layerzero in deployments"))?;

    let template_path = templates_root
        .join("oz-monitor")
        .join("monitors")
        .join("layerzero_job_assigned.json");
    let mut monitor = read_json_value(&template_path)?;
    monitor["addresses"][0]["address"] = Value::String(address);
    if !env_config.is_local() {
        monitor["networks"] = json!([format!("chain_{}", env_config.chains.source.chain_id)]);
    }

    write_pretty_json(
        &generated_dir
            .join("oz-monitor")
            .join("monitors")
            .join("layerzero_job_assigned.json"),
        &monitor,
    )
}

fn refresh_relay_infra_deploy_data_from_broadcast(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<genesis::RelayInfraAddresses> {
    let deployments = DeploymentsConfig::load_or_default(&context.deployments)?;
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
        "vaultFactory": relay_infra_address(
            &broadcast,
            env_config,
            &deployments,
            "VaultFactory",
            "symbioticCore.vaultFactory",
            Some(("symbioticCore", "vaultFactory")),
            "destination symbiotic core vaultFactory",
        )?,
        "operatorRegistry": relay_infra_address(
            &broadcast,
            env_config,
            &deployments,
            "OperatorRegistry",
            "symbioticCore.operatorRegistry",
            Some(("symbioticCore", "operatorRegistry")),
            "destination symbiotic core operatorRegistry",
        )?,
        "networkRegistry": relay_infra_address(
            &broadcast,
            env_config,
            &deployments,
            "NetworkRegistry",
            "symbioticCore.networkRegistry",
            Some(("symbioticCore", "networkRegistry")),
            "destination symbiotic core networkRegistry",
        )?,
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
    broadcast_created_address_optional(broadcast, contract_name)
        .ok_or_else(|| eyre!("missing CREATE address for {contract_name} in relay broadcast"))
}

fn broadcast_created_address_optional(broadcast: &Value, contract_name: &str) -> Option<String> {
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
}

fn relay_infra_address(
    broadcast: &Value,
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    contract_name: &str,
    deployment_key: &str,
    fallback: Option<(&str, &str)>,
    label: &str,
) -> Result<String> {
    if let Some(address) = broadcast_created_address_optional(broadcast, contract_name) {
        return Ok(address);
    }

    addresses::require(
        env_config,
        deployments,
        ChainRole::Destination,
        deployment_key,
        fallback,
        label,
    )
}

fn write_layerzero_endpoint_files(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<()> {
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
        (
            "LAYERZERO_OAPP_ENABLED".to_string(),
            env_config.layerzero_oapp_enabled().to_string(),
        ),
    ];

    for (i, signer) in env_config
        .operator_signers(&context.project_root, &context.env_name)?
        .iter()
        .enumerate()
    {
        envs.push((
            format!("OPERATOR_{}_PRIVATE_KEY", i + 1),
            signer.private_key.clone(),
        ));
    }
    envs.extend(signers::signer_address_envs(context)?);

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

    for (i, signer) in env_config
        .operator_signers(&context.project_root, &context.env_name)?
        .iter()
        .enumerate()
    {
        envs.push((
            format!("OPERATOR_{}_PRIVATE_KEY", i + 1),
            signer.private_key.clone(),
        ));
    }
    envs.extend(signers::signer_address_envs(context)?);

    envs.push((
        "OPERATOR_FUND_AMOUNT".to_string(),
        env_config.funding.operator_amount_wei.clone(),
    ));
    envs.push((
        "RELAYER_SIGNER_FUND_AMOUNT".to_string(),
        env_config.funding.signer_amount_wei.clone(),
    ));
    envs.push((
        "EXTERNAL_MIN_NATIVE_BALANCE".to_string(),
        env_config.funding.min_balance_threshold_wei.clone(),
    ));

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
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<(String, String)> {
    let deployments = DeploymentsConfig::load_or_default(&context.deployments)?;
    let deploy_data = contracts_deploy_data_dir(context);
    fs::create_dir_all(&deploy_data)?;
    let path = deploy_data.join("symbiotic_core.json");
    let body = json!({
        env_config.chains.destination.chain_id.to_string(): {
            "vaultFactory": addresses::require(
                env_config,
                &deployments,
                ChainRole::Destination,
                "symbioticCore.vaultFactory",
                Some(("symbioticCore", "vaultFactory")),
                "destination symbiotic core vaultFactory",
            )?,
            "delegatorFactory": env_config.predeploy(ChainRole::Destination, "symbioticCore", "delegatorFactory")
                .ok_or_else(|| eyre!("missing destination symbiotic core delegatorFactory"))?,
            "slasherFactory": env_config.predeploy(ChainRole::Destination, "symbioticCore", "slasherFactory")
                .ok_or_else(|| eyre!("missing destination symbiotic core slasherFactory"))?,
            "networkRegistry": addresses::require(
                env_config,
                &deployments,
                ChainRole::Destination,
                "symbioticCore.networkRegistry",
                Some(("symbioticCore", "networkRegistry")),
                "destination symbiotic core networkRegistry",
            )?,
            "networkMiddlewareService": env_config.predeploy(ChainRole::Destination, "symbioticCore", "networkMiddlewareService")
                .ok_or_else(|| eyre!("missing destination symbiotic core networkMiddlewareService"))?,
            "operatorRegistry": addresses::require(
                env_config,
                &deployments,
                ChainRole::Destination,
                "symbioticCore.operatorRegistry",
                Some(("symbioticCore", "operatorRegistry")),
                "destination symbiotic core operatorRegistry",
            )?,
            "operatorVaultOptInService": env_config.predeploy(ChainRole::Destination, "symbioticCore", "operatorVaultOptInService")
                .ok_or_else(|| eyre!("missing destination symbiotic core operatorVaultOptInService"))?,
            "operatorNetworkOptInService": env_config.predeploy(ChainRole::Destination, "symbioticCore", "operatorNetworkOptInService")
                .ok_or_else(|| eyre!("missing destination symbiotic core operatorNetworkOptInService"))?,
            "vaultConfigurator": env_config.predeploy(ChainRole::Destination, "symbioticCore", "vaultConfigurator")
                .ok_or_else(|| eyre!("missing destination symbiotic core vaultConfigurator"))?,
        }
    });
    fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&body)?))?;
    Ok((
        "SYMBIOTIC_CORE_CONFIG".to_string(),
        path.display().to_string(),
    ))
}

fn run_layerzero_stack(
    context: &ResolvedContext,
    local: bool,
    envs: &[(String, String)],
) -> Result<()> {
    let mut args = vec![
        "script",
        "script/DeployLayerZeroStack.s.sol:DeployLayerZeroStack",
        "--sig",
        if local {
            "deployLocal()"
        } else {
            "deployExternal()"
        },
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
    // Resume from prior broadcast if one exists for this script (handles
    // failed mid-deploy runs, e.g. deployer ran out of gas).
    let multi_broadcast = context
        .project_root
        .join("contracts")
        .join("broadcast")
        .join("multi")
        .join("DeployLayerZeroStack.s.sol-latest");
    if !local && multi_broadcast.exists() {
        args.push("--resume");
    }
    args.push("--quiet");

    run_contracts_command(context, "forge", &args, envs)
}

fn mine_block(rpc_url: &str) -> Result<()> {
    crate::eth::AlloyEth.mine_block(rpc_url)
}

fn deploy_relay_infra_with_retries(
    context: &ResolvedContext,
    dest_chain_id: u64,
    dest_rpc: &str,
    private_key: &str,
    envs: &[(String, String)],
) -> Result<()> {
    let timeout =
        runtime::setting(context, "FORGE_BROADCAST_TIMEOUT").unwrap_or_else(|| "180".to_string());
    let mut last_error = None;
    for attempt in 1..=3 {
        // Resume from prior broadcast if one exists (handles dropped txs from
        // RPC flakiness without requiring a manual restart pass).
        let can_resume = relay_broadcast_exists(context, dest_chain_id);
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
        if can_resume {
            args.push("--resume");
        }
        match run_contracts_command(context, "forge", &args, envs) {
            Ok(()) => return Ok(()),
            Err(err) => {
                let message = err.to_string();
                let is_nonce_error = message.contains("nonce")
                    || message.contains("Nonce")
                    || message.contains("already known");

                if attempt < 3 {
                    if can_resume {
                        ui::warn(&format!(
                            "deploy attempt {attempt} failed; retrying with --resume"
                        ));
                    } else if is_nonce_error {
                        ui::warn(&format!(
                            "deploy attempt {attempt} failed (nonce conflict); waiting for mempool to clear"
                        ));
                        std::thread::sleep(std::time::Duration::from_secs(15));
                    } else {
                        ui::warn(&format!("deploy attempt {attempt} failed; retrying"));
                    }
                }
                last_error = Some(message);
            }
        }
    }
    bail!(
        "relay infrastructure deploy failed after 3 attempts: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
}

/// Check for an existing broadcast file (not dry-run) that `forge script --resume` can use.
fn relay_broadcast_exists(context: &ResolvedContext, dest_chain_id: u64) -> bool {
    context
        .project_root
        .join("contracts")
        .join("broadcast")
        .join("DeployRelayInfra.s.sol")
        .join(dest_chain_id.to_string())
        .join("run-latest.json")
        .exists()
}

fn contracts_deploy_data_dir(context: &ResolvedContext) -> std::path::PathBuf {
    context.project_root.join("contracts").join("deploy-data")
}

fn checkpoint_deployment_state(context: &ResolvedContext) -> Result<()> {
    publish::publish(context)?;
    Ok(())
}

fn clear_example_oapp_deploy_data(context: &ResolvedContext) -> Result<()> {
    for path in [
        contracts_deploy_data_dir(context).join("example_oapp_source.json"),
        contracts_deploy_data_dir(context).join("example_oapp_dest.json"),
        contracts_deploy_data_dir(context).join("testoapp_source.json"),
        contracts_deploy_data_dir(context).join("testoapp_dest.json"),
    ] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn run_contracts_command(
    context: &ResolvedContext,
    program: &str,
    args: &[&str],
    envs: &[(String, String)],
) -> Result<()> {
    let mut command = Command::new(program);
    command
        .current_dir(context.project_root.join("contracts"))
        .args(args)
        .envs(envs.iter().map(|(key, value)| (key, value)));
    let output = ui::run_command(&mut command, &format!("still running {program}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(ui::command_failure(
            &format!("{program} {}", args.join(" ")),
            &output
        )))
    }
}

fn require_deployment(value: Option<String>, message: &str, failures: &mut Vec<String>) {
    if value.as_deref().is_none_or(str::is_empty) {
        failures.push(message.to_string());
    }
}

fn check_code<E: EthApi>(
    rpc_url: Option<&str>,
    address: Option<&str>,
    label: &str,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let Some(rpc_url) = rpc_url else {
        return;
    };
    let Some(address) = address else {
        failures.push(format!("missing {label} deployment"));
        return;
    };
    let Some(address) = parse_address(address) else {
        failures.push(format!("invalid {label} address: {address}"));
        return;
    };

    match eth.has_code(rpc_url, address) {
        Ok(true) => {}
        Ok(false) => failures.push(format!("{label} has no code at {address}")),
        Err(err) => failures.push(format!("failed to check code for {label}: {err}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::context::ResolvedContext;

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
                },
                "funding": {
                    "operatorAmountWei": "1000000000000000000",
                    "signerAmountWei": "1000000000000000000",
                    "minBalanceThresholdWei": "1000000000000000000"
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
        std::mem::forget(temp_dir); // keep temp dir alive for test duration

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
        assert_eq!(
            relay.settlement,
            "0x4444444444444444444444444444444444444444"
        );

        let written = fs::read_to_string(deploy_data_dir.join("relay_infra.json")).unwrap();
        let json: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(json["driver"], "0x5555555555555555555555555555555555555555");
        assert_eq!(
            json["settlement"],
            "0x4444444444444444444444444444444444444444"
        );
        assert_eq!(
            json["networkRegistry"],
            "0x9999999999999999999999999999999999999999"
        );
    }

    #[test]
    fn refreshes_relay_infra_deploy_data_from_broadcast_with_predeployed_core() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join("testnet.json");
        let deployments = root.join("deployments.json");
        let broadcast_dir = root
            .join("contracts")
            .join("broadcast")
            .join("DeployRelayInfra.s.sol")
            .join("11155111");
        let deploy_data_dir = root.join("contracts").join("deploy-data");
        fs::create_dir_all(&broadcast_dir).unwrap();
        fs::create_dir_all(&deploy_data_dir).unwrap();

        fs::write(
            &env_config,
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "base-sepolia", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": {
                        "name": "sepolia",
                        "chainId": 11155111,
                        "eid": 40161,
                        "confirmations": 3,
                        "blockTimeMs": 12000,
                        "predeploys": {
                            "symbioticCore": {
                                "vaultFactory": "0x7777777777777777777777777777777777777777",
                                "operatorRegistry": "0x8888888888888888888888888888888888888888",
                                "networkRegistry": "0x9999999999999999999999999999999999999999"
                            }
                        }
                    }
                },
                "relay": {
                    "epochDurationSeconds": 28800,
                    "slashingWindowSeconds": 86400,
                    "epochStartDelaySeconds": 3600
                },
                "funding": {
                    "operatorAmountWei": "10000000000000000",
                    "signerAmountWei": "10000000000000000",
                    "minBalanceThresholdWei": "5000000000000000"
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
                    { "contractName": "MockERC20", "transactionType": "CREATE", "contractAddress": "0x6666666666666666666666666666666666666666" }
                ]
            }"#,
        )
        .unwrap();
        std::mem::forget(temp_dir); // keep temp dir alive for test duration

        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "testnet".to_string(),
            env_config,
            deployments,
            generated_dir: root.join("generated").join("testnet"),
        };
        let env = EnvironmentConfig::load(&context.env_config).unwrap();

        let relay = refresh_relay_infra_deploy_data_from_broadcast(&context, &env).unwrap();
        assert_eq!(relay.driver, "0x5555555555555555555555555555555555555555");
        assert_eq!(
            relay.settlement,
            "0x4444444444444444444444444444444444444444"
        );

        let written = fs::read_to_string(deploy_data_dir.join("relay_infra.json")).unwrap();
        let json: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            json["vaultFactory"],
            "0x7777777777777777777777777777777777777777"
        );
        assert_eq!(
            json["operatorRegistry"],
            "0x8888888888888888888888888888888888888888"
        );
        assert_eq!(
            json["networkRegistry"],
            "0x9999999999999999999999999999999999999999"
        );
    }

    #[test]
    fn relay_broadcast_exists_checks_broadcast_dir() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "testnet".to_string(),
            env_config: root.join("testnet.json"),
            deployments: root.join("deployments.json"),
            generated_dir: root.join("generated").join("testnet"),
        };

        assert!(!relay_broadcast_exists(&context, 11155111));

        let broadcast_dir = root
            .join("contracts")
            .join("broadcast")
            .join("DeployRelayInfra.s.sol")
            .join("11155111");
        fs::create_dir_all(&broadcast_dir).unwrap();
        fs::write(broadcast_dir.join("run-latest.json"), "{}").unwrap();

        assert!(relay_broadcast_exists(&context, 11155111));
    }

    #[test]
    fn checkpoint_deployment_state_publishes_contracts_before_genesis() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let deploy_data_dir = root.join("contracts").join("deploy-data");
        fs::create_dir_all(&deploy_data_dir).unwrap();

        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "testnet".to_string(),
            env_config: root.join("testnet.json"),
            deployments: root.join("deployments").join("testnet.json"),
            generated_dir: root.join("generated").join("testnet"),
        };

        fs::write(
            deploy_data_dir.join("source_contracts.json"),
            r#"{ "dvn": "0x1111111111111111111111111111111111111111" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data_dir.join("dest_contracts.json"),
            r#"{ "dvn": "0x2222222222222222222222222222222222222222" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data_dir.join("relay_infra.json"),
            r#"{
                "settlement": "0x3333333333333333333333333333333333333333",
                "driver": "0x4444444444444444444444444444444444444444",
                "keyRegistry": "0x5555555555555555555555555555555555555555",
                "votingPowers": "0x6666666666666666666666666666666666666666",
                "network": "0x7777777777777777777777777777777777777777",
                "stakingToken": "0x8888888888888888888888888888888888888888"
            }"#,
        )
        .unwrap();
        fs::write(
            deploy_data_dir.join("example_oapp_source.json"),
            r#"{ "oapp": "0x9999999999999999999999999999999999999999" }"#,
        )
        .unwrap();

        checkpoint_deployment_state(&context).unwrap();

        let deployments: Value =
            serde_json::from_str(&fs::read_to_string(&context.deployments).unwrap()).unwrap();
        assert_eq!(
            deployments["source"]["dvn"].as_str(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            deployments["destination"]["relayInfra"]["settlement"].as_str(),
            Some("0x3333333333333333333333333333333333333333")
        );
        assert_eq!(
            deployments["layerzero"]["oapp"]["source"].as_str(),
            Some("0x9999999999999999999999999999999999999999")
        );
    }

    #[test]
    fn symbiotic_core_config_is_written_under_contracts_deploy_data() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join("testnet.json");
        let deployments = root.join("deployments.json");

        fs::write(
            &env_config,
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "base-sepolia", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": {
                        "name": "sepolia",
                        "chainId": 11155111,
                        "eid": 40161,
                        "confirmations": 3,
                        "blockTimeMs": 12000,
                        "predeploys": {
                            "symbioticCore": {
                                "vaultFactory": "0x7777777777777777777777777777777777777777",
                                "delegatorFactory": "0x1111111111111111111111111111111111111111",
                                "slasherFactory": "0x2222222222222222222222222222222222222222",
                                "networkRegistry": "0x9999999999999999999999999999999999999999",
                                "networkMiddlewareService": "0x3333333333333333333333333333333333333333",
                                "operatorRegistry": "0x8888888888888888888888888888888888888888",
                                "operatorVaultOptInService": "0x4444444444444444444444444444444444444444",
                                "operatorNetworkOptInService": "0x5555555555555555555555555555555555555555",
                                "vaultConfigurator": "0x6666666666666666666666666666666666666666"
                            }
                        }
                    }
                },
                "funding": {
                    "operatorAmountWei": "10000000000000000",
                    "signerAmountWei": "10000000000000000",
                    "minBalanceThresholdWei": "5000000000000000"
                }
            }"#,
        )
        .unwrap();
        fs::write(&deployments, r#"{ "source": {}, "destination": {} }"#).unwrap();

        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "testnet".to_string(),
            env_config,
            deployments,
            generated_dir: root.join("generated").join("testnet"),
        };
        let env = EnvironmentConfig::load(&context.env_config).unwrap();

        let (_, written_path) = symbiotic_core_config(&context, &env).unwrap();
        assert!(written_path.ends_with("contracts/deploy-data/symbiotic_core.json"));
        assert!(
            root.join("contracts/deploy-data/symbiotic_core.json")
                .exists()
        );
    }
}
