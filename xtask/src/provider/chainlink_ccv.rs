use std::env;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;

use alloy::providers::ProviderBuilder;
use alloy::sol;
use eyre::{Result, bail, eyre};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::addresses;
use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::genesis;
use crate::render::{read_json_value, write_pretty_json};
use crate::runtime::{self, RuntimeInputs};
use crate::signers;
use crate::ui;

sol! {
    #[sol(rpc)]
    interface CcvOnRampReader {
        function nonce() external view returns (uint64);
    }

    #[sol(rpc)]
    interface CcvOffRampReader {
        function sourceChainSelector() external view returns (uint64);
    }
}

pub fn deploy(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    let runtime = RuntimeInputs::resolve(context, env_config);
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
    let deployer_address = AlloyEth.address_from_private_key(&private_key)?.to_string();
    let selectors = chain_selectors(context, env_config)?;

    fs::create_dir_all(contracts_deploy_data_dir(context))?;

    let source_relay = ui::step("deploy source relay infrastructure");
    run_relay_infra(
        context,
        env_config,
        ChainRole::Source,
        &source_rpc,
        &private_key,
    )?;
    snapshot_source_relay_infra(context)?;
    source_relay.done("source relay infrastructure deployed");

    let dest_relay = ui::step("deploy destination relay infrastructure");
    run_relay_infra(
        context,
        env_config,
        ChainRole::Destination,
        &dest_rpc,
        &private_key,
    )?;
    dest_relay.done("destination relay infrastructure deployed");

    let source_settlement = read_settlement(&source_relay_infra_path(context))?;
    let dest_settlement = read_settlement(&dest_relay_infra_path(context))?;

    let ccv = ui::step("deploy ccv contracts");
    run_deploy_ccv(
        context,
        &source_rpc,
        &dest_rpc,
        &private_key,
        &deployer_address,
        &source_settlement,
        &dest_settlement,
        &selectors,
    )?;
    ccv.done("ccv contracts deployed");

    if env_config.is_local() {
        let blocks = ui::step("mine local blocks");
        AlloyEth.mine_block(&source_rpc)?;
        AlloyEth.mine_block(&dest_rpc)?;
        blocks.done("local blocks mined");
    }

    let relay = dest_relay_addresses(context)?;
    let genesis = ui::step("commit settlement genesis");
    genesis::ensure_genesis_for_relay(context, env_config, &relay, false, env_config.is_local())?;
    genesis.done("settlement genesis committed");

    Ok(())
}

pub fn validate_chain_state<E: EthApi>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let src_ccv = deployments.deployment(ChainRole::Source, "chainlinkCcv.ccv");
    let dst_ccv = deployments.deployment(ChainRole::Destination, "chainlinkCcv.ccv");
    let src_onramp = deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp");
    let dst_offramp = deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp");
    let settlement = deployments.deployment(ChainRole::Destination, "chainlinkCcv.settlement");

    check_code(
        runtime.source_rpc.as_deref(),
        src_ccv.as_deref(),
        "source CCV",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_ccv.as_deref(),
        "destination CCV",
        eth,
        failures,
    );
    check_code(
        runtime.source_rpc.as_deref(),
        src_onramp.as_deref(),
        "source onRamp",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_offramp.as_deref(),
        "destination offRamp",
        eth,
        failures,
    );

    if let Some(settlement) = settlement.as_deref()
        && !settlement.is_empty()
    {
        check_code(
            runtime.dest_rpc.as_deref(),
            Some(settlement),
            "destination CCV settlement",
            eth,
            failures,
        );
        if let (Some(dest_rpc), Some(dst_ccv)) = (runtime.dest_rpc.as_deref(), dst_ccv.as_deref()) {
            let actual = parse_address(dst_ccv)
                .and_then(|address| eth.settlement_address(dest_rpc, address).ok())
                .map(|value| value.to_string());
            if let Some(actual) = actual
                && !actual.eq_ignore_ascii_case(settlement)
            {
                failures.push(format!(
                    "destination CCV settlement mismatch: expected {settlement}, got {}",
                    actual.to_ascii_lowercase()
                ));
            }
        }
    }
}

pub fn validate_configuration(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    failures: &mut Vec<String>,
) {
    require_deployment(
        deployments.deployment(ChainRole::Source, "chainlinkCcv.ccv"),
        "missing source CCV deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "chainlinkCcv.ccv"),
        "missing destination CCV deployment in deployments file",
        failures,
    );

    validate_chain_selector(
        "CCV_SOURCE_CHAIN_SELECTOR",
        env_config.chains.source.chain_id,
        "source chain selector",
        failures,
    );
    validate_chain_selector(
        "CCV_DEST_CHAIN_SELECTOR",
        env_config.chains.destination.chain_id,
        "destination chain selector",
        failures,
    );

    let addresses = [
        (
            resolve_env_or_deployment(
                "CCV_SOURCE_ONRAMP_ADDRESS",
                deployments,
                ChainRole::Source,
                "chainlinkCcv.onRamp",
            ),
            "missing CCV source onRamp. Set CCV_SOURCE_ONRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV source onRamp address",
        ),
        (
            resolve_env_or_deployment(
                "CCV_SOURCE_OFFRAMP_ADDRESS",
                deployments,
                ChainRole::Source,
                "chainlinkCcv.offRamp",
            ),
            "missing CCV source offRamp. Set CCV_SOURCE_OFFRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV source offRamp address",
        ),
        (
            resolve_env_or_deployment(
                "CCV_DEST_ONRAMP_ADDRESS",
                deployments,
                ChainRole::Destination,
                "chainlinkCcv.onRamp",
            ),
            "missing CCV destination onRamp. Set CCV_DEST_ONRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV destination onRamp address",
        ),
        (
            resolve_env_or_deployment(
                "CCV_DEST_OFFRAMP_ADDRESS",
                deployments,
                ChainRole::Destination,
                "chainlinkCcv.offRamp",
            ),
            "missing CCV destination offRamp. Set CCV_DEST_OFFRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV destination offRamp address",
        ),
    ];

    for (value, missing_message, invalid_prefix) in addresses {
        match value {
            Some(address) if is_hex_address(&address) => {}
            Some(address) => failures.push(format!("{invalid_prefix}: {address}")),
            None => failures.push(missing_message.to_string()),
        }
    }

    if !env_config.is_local() {
        for (role, role_label) in [
            (ChainRole::Source, "source"),
            (ChainRole::Destination, "destination"),
        ] {
            for key in ["vaultFactory", "operatorRegistry", "networkRegistry"] {
                let label = format!("{role_label} symbiotic core {key}");
                if let Err(err) = addresses::require(
                    env_config,
                    deployments,
                    role,
                    &format!("symbioticCore.{key}"),
                    Some(("symbioticCore", key)),
                    &label,
                ) {
                    failures.push(err.to_string());
                }
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
    let address = env::var("CCV_SOURCE_ONRAMP_ADDRESS")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp"))
        .ok_or_else(|| {
            eyre!("missing monitor address for provider chainlink_ccv in deployments/env overrides")
        })?;

    let template_path = templates_root
        .join("oz-monitor")
        .join("monitors")
        .join("ccip_message_sent.json");
    let mut monitor = read_json_value(&template_path)?;
    monitor["addresses"][0]["address"] = Value::String(address);
    if !env_config.is_local() {
        monitor["networks"] = json!([format!("chain_{}", env_config.chains.source.chain_id)]);
    }

    write_pretty_json(
        &generated_dir
            .join("oz-monitor")
            .join("monitors")
            .join("ccip_message_sent.json"),
        &monitor,
    )
}

pub fn configure_startup(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    let deployments = DeploymentsConfig::load(&context.deployments)?;
    let runtime = RuntimeInputs::resolve(context, env_config);
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
    let deployer_address = AlloyEth.address_from_private_key(&private_key)?.to_string();
    let selectors = chain_selectors(context, env_config)?;
    let config = configure_inputs(context, &deployments)?;

    ensure_mock_contract(&source_rpc, config.source_onramp, "source onRamp")?;
    ensure_mock_contract(&source_rpc, config.source_offramp, "source offRamp")?;
    ensure_mock_contract(&dest_rpc, config.dest_onramp, "destination onRamp")?;
    ensure_mock_contract(&dest_rpc, config.dest_offramp, "destination offRamp")?;
    ensure_onramp_reachable(&source_rpc, config.source_onramp, "source onRamp")?;
    ensure_offramp_reachable(&source_rpc, config.source_offramp, "source offRamp")?;
    ensure_onramp_reachable(&dest_rpc, config.dest_onramp, "destination onRamp")?;
    ensure_offramp_reachable(&dest_rpc, config.dest_offramp, "destination offRamp")?;

    run_configure_ccv(
        context,
        &source_rpc,
        &private_key,
        &deployer_address,
        config.source_ccv,
        selectors.destination,
        config.source_onramp,
        config.source_offramp,
    )?;
    run_configure_ccv(
        context,
        &dest_rpc,
        &private_key,
        &deployer_address,
        config.dest_ccv,
        selectors.source,
        config.dest_onramp,
        config.dest_offramp,
    )?;

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ChainSelectors {
    source: u64,
    destination: u64,
}

#[derive(Debug, Clone, Copy)]
struct ConfigureInputs {
    source_ccv: alloy::primitives::Address,
    dest_ccv: alloy::primitives::Address,
    source_onramp: alloy::primitives::Address,
    source_offramp: alloy::primitives::Address,
    dest_onramp: alloy::primitives::Address,
    dest_offramp: alloy::primitives::Address,
}

fn chain_selectors(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<ChainSelectors> {
    Ok(ChainSelectors {
        source: runtime::setting(context, "CCV_SOURCE_CHAIN_SELECTOR")
            .unwrap_or_else(|| env_config.chains.source.chain_id.to_string())
            .parse()?,
        destination: runtime::setting(context, "CCV_DEST_CHAIN_SELECTOR")
            .unwrap_or_else(|| env_config.chains.destination.chain_id.to_string())
            .parse()?,
    })
}

fn configure_inputs(
    context: &ResolvedContext,
    deployments: &DeploymentsConfig,
) -> Result<ConfigureInputs> {
    Ok(ConfigureInputs {
        source_ccv: resolve_address(
            context,
            "CCV_SOURCE_ADDRESS",
            deployments.deployment(ChainRole::Source, "chainlinkCcv.ccv"),
            "source SymbioticCCV",
        )?,
        dest_ccv: resolve_address(
            context,
            "CCV_DEST_ADDRESS",
            deployments.deployment(ChainRole::Destination, "chainlinkCcv.ccv"),
            "destination SymbioticCCV",
        )?,
        source_onramp: resolve_address(
            context,
            "CCV_SOURCE_ONRAMP_ADDRESS",
            deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp"),
            "source onRamp",
        )?,
        source_offramp: resolve_address(
            context,
            "CCV_SOURCE_OFFRAMP_ADDRESS",
            deployments.deployment(ChainRole::Source, "chainlinkCcv.offRamp"),
            "source offRamp",
        )?,
        dest_onramp: resolve_address(
            context,
            "CCV_DEST_ONRAMP_ADDRESS",
            deployments.deployment(ChainRole::Destination, "chainlinkCcv.onRamp"),
            "destination onRamp",
        )?,
        dest_offramp: resolve_address(
            context,
            "CCV_DEST_OFFRAMP_ADDRESS",
            deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp"),
            "destination offRamp",
        )?,
    })
}

fn resolve_address(
    context: &ResolvedContext,
    env_key: &str,
    fallback: Option<String>,
    label: &str,
) -> Result<alloy::primitives::Address> {
    let value = runtime::setting(context, env_key)
        .filter(|value| !value.is_empty())
        .or(fallback)
        .ok_or_else(|| eyre!("missing {label}"))?;
    parse_address(&value).ok_or_else(|| eyre!("invalid {label} address: {value}"))
}

fn run_relay_infra(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    role: ChainRole,
    rpc_url: &str,
    private_key: &str,
) -> Result<()> {
    let envs = relay_envs(context, env_config, role)?;
    let timeout =
        runtime::setting(context, "FORGE_BROADCAST_TIMEOUT").unwrap_or_else(|| "180".to_string());
    let mut last_error = None;

    for attempt in 1..=3 {
        let mut args = vec![
            "script".to_string(),
            "script/DeployRelayInfra.s.sol:DeployRelayInfra".to_string(),
            "--rpc-url".to_string(),
            rpc_url.to_string(),
            "--broadcast".to_string(),
            "--private-key".to_string(),
            private_key.to_string(),
            "--code-size-limit".to_string(),
            "50000".to_string(),
            "--gas-estimate-multiplier".to_string(),
            if attempt > 1 {
                "200".to_string()
            } else {
                "150".to_string()
            },
            "--timeout".to_string(),
            timeout.clone(),
            "--slow".to_string(),
            "--non-interactive".to_string(),
            "--quiet".to_string(),
        ];
        if attempt > 1 {
            args.push("--resume".to_string());
        }

        match run_forge(context, &args, &envs) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_error = Some(err.to_string());
                if attempt < 3 {
                    ui::warn(&format!(
                        "relay infrastructure deploy attempt {attempt} failed; retrying with --resume"
                    ));
                }
            }
        }
    }

    bail!(
        "relay infrastructure deploy failed after 3 attempts: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
}

#[allow(clippy::too_many_arguments)]
fn run_deploy_ccv(
    context: &ResolvedContext,
    source_rpc: &str,
    dest_rpc: &str,
    private_key: &str,
    deployer_address: &str,
    source_settlement: &str,
    dest_settlement: &str,
    selectors: &ChainSelectors,
) -> Result<()> {
    let common_envs = vec![("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string())];
    let dest_selector = selectors.destination.to_string();
    let source_selector = selectors.source.to_string();

    let source_args = vec![
        "script".to_string(),
        "script/DeployCCV.s.sol:DeployCCV".to_string(),
        "--sig".to_string(),
        "deploySource(address,uint64)".to_string(),
        source_settlement.to_string(),
        dest_selector,
        "--rpc-url".to_string(),
        source_rpc.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &source_args, &common_envs)?;

    let dest_args = vec![
        "script".to_string(),
        "script/DeployCCV.s.sol:DeployCCV".to_string(),
        "--sig".to_string(),
        "deployDest(address,uint64)".to_string(),
        dest_settlement.to_string(),
        source_selector,
        "--rpc-url".to_string(),
        dest_rpc.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &dest_args, &common_envs)
}

#[allow(clippy::too_many_arguments)]
fn run_configure_ccv(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
    ccv: alloy::primitives::Address,
    remote_selector: u64,
    onramp: alloy::primitives::Address,
    offramp: alloy::primitives::Address,
) -> Result<()> {
    let mut envs = vec![
        ("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string()),
        (
            "CCV_REMOTE_CHAIN_SELECTOR".to_string(),
            remote_selector.to_string(),
        ),
        ("CCV_ONRAMP_ADDRESS".to_string(), onramp.to_string()),
        ("CCV_OFFRAMP_ADDRESS".to_string(), offramp.to_string()),
    ];
    for key in [
        "CCV_ALLOWLIST_ENABLED",
        "CCV_FEE_USD_CENTS",
        "CCV_GAS_FOR_VERIFICATION",
        "CCV_PAYLOAD_SIZE_BYTES",
    ] {
        if let Some(value) = runtime::setting(context, key).filter(|value| !value.is_empty()) {
            envs.push((key.to_string(), value));
        }
    }

    let args = vec![
        "script".to_string(),
        "script/ConfigureCCV.s.sol:ConfigureCCV".to_string(),
        "--sig".to_string(),
        "run(address)".to_string(),
        ccv.to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &args, &envs)
}

fn snapshot_source_relay_infra(context: &ResolvedContext) -> Result<()> {
    let source = dest_relay_infra_path(context);
    let target = source_relay_infra_path(context);
    fs::copy(&source, &target).map_err(|err| {
        eyre!(
            "failed to snapshot source relay infra {} -> {}: {err}",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn read_settlement(path: &Path) -> Result<String> {
    let json = read_json_value(path)?;
    json.get("settlement")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| eyre!("missing settlement in {}", path.display()))
}

fn dest_relay_addresses(context: &ResolvedContext) -> Result<genesis::RelayInfraAddresses> {
    let json = read_json_value(&dest_relay_infra_path(context))?;
    Ok(genesis::RelayInfraAddresses {
        driver: json
            .get("driver")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                eyre!(
                    "missing driver in {}",
                    dest_relay_infra_path(context).display()
                )
            })?
            .to_string(),
        settlement: json
            .get("settlement")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                eyre!(
                    "missing settlement in {}",
                    dest_relay_infra_path(context).display()
                )
            })?
            .to_string(),
    })
}

fn relay_envs(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    role: ChainRole,
) -> Result<Vec<(String, String)>> {
    let deployments = DeploymentsConfig::load_or_default(&context.deployments)?;
    let mut envs = vec![
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
    ];

    if !env_config.is_local()
        && let Some(core) = symbiotic_core_config(env_config, &deployments, role)?
    {
        envs.push(("SYMBIOTIC_CORE_CONFIG".to_string(), core));
    }

    for index in 0..3 {
        let key = runtime::operator_private_key(context, index)
            .ok_or_else(|| eyre!("OPERATOR_{}_PRIVATE_KEY is not set", index + 1))?;
        envs.push((format!("OPERATOR_{}_PRIVATE_KEY", index + 1), key));
    }
    envs.extend(signers::signer_address_envs(context)?);

    if let Some(value) =
        runtime::setting(context, "RELAYER_SIGNER_FUND_AMOUNT").filter(|value| !value.is_empty())
    {
        envs.push(("RELAYER_SIGNER_FUND_AMOUNT".to_string(), value));
    }

    Ok(envs)
}

fn symbiotic_core_config(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    role: ChainRole,
) -> Result<Option<String>> {
    let chain = env_config.chain(role);
    if chain.predeploys.get("symbioticCore").is_none() && !deployments.role_has_entries(role) {
        return Ok(None);
    }
    let temp = NamedTempFile::new()?;
    let body = json!({
        chain.chain_id.to_string(): {
            "vaultFactory": addresses::require(
                env_config,
                deployments,
                role,
                "symbioticCore.vaultFactory",
                Some(("symbioticCore", "vaultFactory")),
                &format!("{} symbiotic core vaultFactory", role_label(role)),
            )?,
            "delegatorFactory": env_config.predeploy(role, "symbioticCore", "delegatorFactory")
                .ok_or_else(|| eyre!("missing {} symbiotic core delegatorFactory", role_label(role)))?,
            "slasherFactory": env_config.predeploy(role, "symbioticCore", "slasherFactory")
                .ok_or_else(|| eyre!("missing {} symbiotic core slasherFactory", role_label(role)))?,
            "networkRegistry": addresses::require(
                env_config,
                deployments,
                role,
                "symbioticCore.networkRegistry",
                Some(("symbioticCore", "networkRegistry")),
                &format!("{} symbiotic core networkRegistry", role_label(role)),
            )?,
            "networkMiddlewareService": env_config.predeploy(role, "symbioticCore", "networkMiddlewareService")
                .ok_or_else(|| eyre!("missing {} symbiotic core networkMiddlewareService", role_label(role)))?,
            "operatorRegistry": addresses::require(
                env_config,
                deployments,
                role,
                "symbioticCore.operatorRegistry",
                Some(("symbioticCore", "operatorRegistry")),
                &format!("{} symbiotic core operatorRegistry", role_label(role)),
            )?,
            "operatorVaultOptInService": env_config.predeploy(role, "symbioticCore", "operatorVaultOptInService")
                .ok_or_else(|| eyre!("missing {} symbiotic core operatorVaultOptInService", role_label(role)))?,
            "operatorNetworkOptInService": env_config.predeploy(role, "symbioticCore", "operatorNetworkOptInService")
                .ok_or_else(|| eyre!("missing {} symbiotic core operatorNetworkOptInService", role_label(role)))?,
            "vaultConfigurator": env_config.predeploy(role, "symbioticCore", "vaultConfigurator")
                .ok_or_else(|| eyre!("missing {} symbiotic core vaultConfigurator", role_label(role)))?,
        }
    });
    fs::write(
        temp.path(),
        format!("{}\n", serde_json::to_string_pretty(&body)?),
    )?;
    let (_file, path) = temp.keep()?;
    Ok(Some(path.display().to_string()))
}

fn role_label(role: ChainRole) -> &'static str {
    match role {
        ChainRole::Source => "source",
        ChainRole::Destination => "destination",
    }
}

fn run_forge(context: &ResolvedContext, args: &[String], envs: &[(String, String)]) -> Result<()> {
    let mut command = Command::new("forge");
    command
        .current_dir(context.project_root.join("contracts"))
        .args(args)
        .envs(envs.iter().map(|(key, value)| (key, value)));
    let output = ui::run_command(&mut command, "still running forge")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(ui::command_failure(
            &format!("forge {}", args.join(" ")),
            &output
        )))
    }
}

fn ensure_mock_contract(
    rpc_url: &str,
    address: alloy::primitives::Address,
    label: &str,
) -> Result<()> {
    if AlloyEth.has_code(rpc_url, address)? {
        Ok(())
    } else {
        bail!("{label} has no code at {address}");
    }
}

fn ensure_onramp_reachable(
    rpc_url: &str,
    address: alloy::primitives::Address,
    label: &str,
) -> Result<()> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
        let contract = CcvOnRampReader::new(address, provider);
        contract.nonce().call().await?;
        Ok(())
    })
    .map_err(|err| eyre!("{label} is not reachable or not CCV mock-compatible: {err}"))
}

fn ensure_offramp_reachable(
    rpc_url: &str,
    address: alloy::primitives::Address,
    label: &str,
) -> Result<()> {
    block_on(async move {
        let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
        let contract = CcvOffRampReader::new(address, provider);
        contract.sourceChainSelector().call().await?;
        Ok(())
    })
    .map_err(|err| eyre!("{label} is not reachable or not CCV mock-compatible: {err}"))
}

fn source_relay_infra_path(context: &ResolvedContext) -> PathBuf {
    contracts_deploy_data_dir(context).join("relay_infra_source.json")
}

fn dest_relay_infra_path(context: &ResolvedContext) -> PathBuf {
    contracts_deploy_data_dir(context).join("relay_infra.json")
}

fn contracts_deploy_data_dir(context: &ResolvedContext) -> PathBuf {
    context.project_root.join("contracts").join("deploy-data")
}

fn require_deployment(value: Option<String>, message: &str, failures: &mut Vec<String>) {
    if value.as_deref().is_none_or(str::is_empty) {
        failures.push(message.to_string());
    }
}

fn validate_chain_selector(env_var: &str, default: u64, label: &str, failures: &mut Vec<String>) {
    match env::var(env_var) {
        Ok(value) if value.is_empty() => failures.push(format!("invalid {label}: ''")),
        Ok(value) if value.parse::<u64>().is_err() => {
            failures.push(format!("invalid {label}: '{value}'"));
        }
        Ok(_) | Err(env::VarError::NotPresent) => {
            let _ = default;
        }
        Err(env::VarError::NotUnicode(_)) => {
            failures.push(format!("invalid {label}: non-utf8 value"))
        }
    }
}

fn resolve_env_or_deployment(
    env_var: &str,
    deployments: &DeploymentsConfig,
    role: ChainRole,
    deployment_key: &str,
) -> Option<String> {
    env::var(env_var)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(role, deployment_key))
}

fn is_hex_address(value: &str) -> bool {
    value.starts_with("0x") && value.len() == 42
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

fn block_on<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}
