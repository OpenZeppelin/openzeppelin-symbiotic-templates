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
use crate::generate::{read_json_value, write_pretty_json};
use crate::genesis;
use crate::publish;
use crate::runtime::{self, RuntimeInputs};
use crate::signers;
use crate::ui;

const CCV_VERSION_TAG: &str = "0x1a75bd93";
const LOCAL_CCV_STORAGE_LOCATION_URIS: &str =
    "http://operator-1:3000,http://operator-2:3000,http://operator-3:3000";

/// Preimage of `DeployCCV.s.sol`'s `RESOLVER_SALT` constant — the CREATE2
/// salt used to deploy `VersionedVerifierResolver` at the same address on
/// every chain.
const RESOLVER_SALT_PREIMAGE: &[u8] = b"symbiotic.ccv.versioned-verifier-resolver.v1";

/// Path (relative to `contracts/`) of the resolver's published creation
/// bytecode, used to derive its deterministic CREATE2 address.
const RESOLVER_BYTECODE_PATH: &str =
    "node_modules/@chainlink/contracts-ccip/bytecode/v2_0_0/versioned_verifier_resolver.bin";

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
    if uses_real_ccip(env_config) {
        deploy_real_ccip(context, env_config)
    } else {
        deploy_with_mocks(context, env_config)
    }
}

/// Original deploy flow for local / mock-based environments.
fn deploy_with_mocks(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
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
    let factory_deployer = env_config.resolve_signer(
        "ccv-factory-deployer",
        &context.project_root,
        &context.env_name,
    )?;
    let factory_deployer_address = factory_deployer.address.to_string();
    let storage_location_uris = ccv_storage_location_uris(context, env_config)?;
    let selectors = chain_selectors(context, env_config)?;

    if env_config.is_local() {
        ensure_factory_deployer_fresh(&AlloyEth, &source_rpc, factory_deployer.address)?;
    }

    fs::create_dir_all(chainlink_deploy_data_dir(context))?;
    fs::create_dir_all(symbiotic_deploy_data_dir(context))?;

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
    let source_session = CcvDeploySession {
        rpc_url: &source_rpc,
        private_key: &private_key,
        deployer_address: &deployer_address,
        factory_private_key: &factory_deployer.private_key,
        factory_deployer_address: &factory_deployer_address,
        storage_location_uris: &storage_location_uris,
    };
    let dest_session = CcvDeploySession {
        rpc_url: &dest_rpc,
        ..source_session
    };
    run_deploy_ccv(
        context,
        &source_session,
        &dest_session,
        &source_settlement,
        &dest_settlement,
        &selectors,
    )?;
    ccv.done("ccv contracts deployed");

    let publish_step = ui::step("checkpoint deployment state");
    checkpoint_deployment_state(context)?;
    publish_step.done("deployment state checkpointed");

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

/// Deploy flow for environments backed by real CCIP staging contracts.
/// Detected by presence of `chainlinkCcip` predeploys. Source-side Symbiotic
/// relay infrastructure is skipped when the source chain has no `symbioticCore`
/// predeploys (dest-only Symbiotic mode); a `NoOpSettlement` stub is deployed
/// instead so `SymbioticVerifier`'s constructor still accepts a non-zero address.
fn deploy_real_ccip(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
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
    let factory_deployer = env_config.resolve_signer(
        "ccv-factory-deployer",
        &context.project_root,
        &context.env_name,
    )?;
    let factory_deployer_address = factory_deployer.address.to_string();
    let storage_location_uris = ccv_storage_location_uris(context, env_config)?;
    let selectors = chain_selectors(context, env_config)?;

    fs::create_dir_all(chainlink_deploy_data_dir(context))?;
    fs::create_dir_all(symbiotic_deploy_data_dir(context))?;

    let source_ccip = chainlink_ccip_predeploys(env_config, ChainRole::Source)?;
    let dest_ccip = chainlink_ccip_predeploys(env_config, ChainRole::Destination)?;

    // Source-side Settlement: NoOpSettlement when source has no symbioticCore predeploys.
    let source_settlement = if has_symbiotic_core(env_config, ChainRole::Source) {
        if let Some(addr) =
            deployed_address(&source_relay_infra_path(context), "settlement", &source_rpc)?
        {
            ui::info(&format!(
                "source relay infrastructure already deployed (settlement {addr}); skipping"
            ));
            addr
        } else {
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
            read_settlement(&source_relay_infra_path(context))?
        }
    } else if let Some(addr) =
        deployed_address(&noop_settlement_path(context), "settlement", &source_rpc)?
    {
        ui::info(&format!(
            "source NoOpSettlement already deployed at {addr}; skipping"
        ));
        addr
    } else {
        let stub = ui::step("deploy source NoOpSettlement");
        let addr =
            run_deploy_noop_settlement(context, &source_rpc, &private_key, &deployer_address)?;
        stub.done(&format!("source NoOpSettlement deployed: {addr}"));
        addr
    };

    // Destination-side: always run full relay infra (this is where verification happens).
    let dest_settlement = if let Some(addr) =
        deployed_address(&dest_relay_infra_path(context), "settlement", &dest_rpc)?
    {
        ui::info(&format!(
            "destination relay infrastructure already deployed (settlement {addr}); skipping"
        ));
        addr
    } else {
        let dest_relay = ui::step("deploy destination relay infrastructure");
        run_relay_infra(
            context,
            env_config,
            ChainRole::Destination,
            &dest_rpc,
            &private_key,
        )?;
        dest_relay.done("destination relay infrastructure deployed");
        read_settlement(&dest_relay_infra_path(context))?
    };

    let ccv = ui::step("deploy CCV resolver and verifier contracts");
    let source_session = CcvDeploySession {
        rpc_url: &source_rpc,
        private_key: &private_key,
        deployer_address: &deployer_address,
        factory_private_key: &factory_deployer.private_key,
        factory_deployer_address: &factory_deployer_address,
        storage_location_uris: &storage_location_uris,
    };
    let dest_session = CcvDeploySession {
        rpc_url: &dest_rpc,
        ..source_session
    };
    let source_target = CcvOnlyChainInputs {
        settlement: &source_settlement,
        ccip: &source_ccip,
        remote_selector: selectors.destination,
    };
    let dest_target = CcvOnlyChainInputs {
        settlement: &dest_settlement,
        ccip: &dest_ccip,
        remote_selector: selectors.source,
    };
    run_deploy_ccv_only(
        context,
        &source_session,
        &dest_session,
        &source_target,
        &dest_target,
    )?;
    ccv.done("CCV resolver and verifier contracts deployed");

    let exec_step = ui::step("deploy source NoOpExecutor");
    let executor_addr =
        run_deploy_noop_executor(context, &source_rpc, &private_key, &deployer_address)?;
    exec_step.done(&format!("NoOpExecutor deployed: {executor_addr}"));

    // ExampleCcipApp references the stable resolver address, not the verifier.
    let source_ccv = read_address(&source_ccv_contracts_path(context), "resolver")?;
    let dest_ccv = read_address(&dest_ccv_contracts_path(context), "resolver")?;

    let app_step = ui::step("deploy ExampleCcipApp on both chains");
    let source_app = run_deploy_example_app(
        context,
        &source_rpc,
        &private_key,
        &deployer_address,
        &source_ccip.router,
        &source_ccv,
        &executor_addr,
        "deploy-data/chainlink/example_app_source.json",
    )?;
    let dest_app = run_deploy_example_app(
        context,
        &dest_rpc,
        &private_key,
        &deployer_address,
        &dest_ccip.router,
        &dest_ccv,
        // Executor address is never used on destination; pass the source NoOpExecutor anyway.
        &executor_addr,
        "deploy-data/chainlink/example_app_dest.json",
    )?;
    app_step.done("ExampleCcipApp deployed on both chains");

    let wire_step = ui::step("wire ExampleCcipApp setRemoteApp on both chains");
    run_set_remote_app(
        context,
        &source_rpc,
        &private_key,
        &deployer_address,
        &source_app,
        selectors.destination,
        &dest_app,
    )?;
    run_set_remote_app(
        context,
        &dest_rpc,
        &private_key,
        &deployer_address,
        &dest_app,
        selectors.source,
        &source_app,
    )?;
    wire_step.done("ExampleCcipApp peers wired");

    let publish_step = ui::step("checkpoint deployment state");
    checkpoint_deployment_state(context)?;
    publish_step.done("deployment state checkpointed");

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
    let src_resolver = deployments.deployment(ChainRole::Source, "chainlinkCcv.resolver");
    let dst_resolver = deployments.deployment(ChainRole::Destination, "chainlinkCcv.resolver");
    let src_verifier = deployments.deployment(ChainRole::Source, "chainlinkCcv.verifier");
    let dst_verifier = deployments.deployment(ChainRole::Destination, "chainlinkCcv.verifier");
    let src_onramp = deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp");
    let dst_offramp = deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp");
    let settlement = deployments.deployment(ChainRole::Destination, "chainlinkCcv.settlement");

    check_code(
        runtime.source_rpc.as_deref(),
        src_resolver.as_deref(),
        "source CCV resolver",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_resolver.as_deref(),
        "destination CCV resolver",
        eth,
        failures,
    );
    check_code(
        runtime.source_rpc.as_deref(),
        src_verifier.as_deref(),
        "source CCV verifier",
        eth,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_verifier.as_deref(),
        "destination CCV verifier",
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
        if let (Some(dest_rpc), Some(dst_verifier)) =
            (runtime.dest_rpc.as_deref(), dst_verifier.as_deref())
        {
            let actual = parse_address(dst_verifier)
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
    _warnings: &mut Vec<String>,
) {
    require_deployment(
        deployments.deployment(ChainRole::Source, "chainlinkCcv.resolver"),
        "missing source CCV resolver deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "chainlinkCcv.resolver"),
        "missing destination CCV resolver deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Source, "chainlinkCcv.verifier"),
        "missing source CCV verifier deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "chainlinkCcv.verifier"),
        "missing destination CCV verifier deployment in deployments file",
        failures,
    );
    for (role, role_label) in [
        (ChainRole::Source, "source"),
        (ChainRole::Destination, "destination"),
    ] {
        for field in ["factory", "router", "rmn"] {
            require_deployment(
                deployments.deployment(role, &format!("chainlinkCcv.{field}")),
                &format!("missing {role_label} CCV {field} deployment in deployments file"),
                failures,
            );
        }
    }

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
            // Dest-only Symbiotic: skip source-side core checks when source has
            // no symbioticCore predeploys.
            if matches!(role, ChainRole::Source) && !has_symbiotic_core(env_config, role) {
                continue;
            }
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
    render_message_sent_monitor(env_config, deployments, templates_root, generated_dir)?;
    render_execution_state_monitor(env_config, deployments, templates_root, generated_dir)?;
    Ok(())
}

fn render_message_sent_monitor(
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

/// Render the destination OffRamp's ExecutionStateChanged monitor — operators
/// consume this to learn the on-chain message outcome (Success/Failure)
/// independently of whose submission tx mined.
fn render_execution_state_monitor(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    templates_root: &Path,
    generated_dir: &Path,
) -> Result<()> {
    let address = env::var("CCV_DEST_OFFRAMP_ADDRESS")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp"))
        .ok_or_else(|| {
            eyre!(
                "missing destination OffRamp address for ExecutionStateChanged monitor in deployments/env overrides"
            )
        })?;

    let template_path = templates_root
        .join("oz-monitor")
        .join("monitors")
        .join("ccip_execution_state_changed.json");
    let mut monitor = read_json_value(&template_path)?;
    monitor["addresses"][0]["address"] = Value::String(address);
    if !env_config.is_local() {
        monitor["networks"] = json!([format!("chain_{}", env_config.chains.destination.chain_id)]);
    }

    write_pretty_json(
        &generated_dir
            .join("oz-monitor")
            .join("monitors")
            .join("ccip_execution_state_changed.json"),
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
    let real_ccip = uses_real_ccip(env_config);

    if real_ccip {
        // Real Chainlink OnRamp/OffRamp contracts: just confirm they have code.
        ensure_has_code(&source_rpc, config.source_onramp, "source onRamp")?;
        ensure_has_code(&dest_rpc, config.dest_offramp, "destination offRamp")?;
    } else {
        ensure_mock_contract(&source_rpc, config.source_onramp, "source onRamp")?;
        ensure_mock_contract(&source_rpc, config.source_offramp, "source offRamp")?;
        ensure_mock_contract(&dest_rpc, config.dest_onramp, "destination onRamp")?;
        ensure_mock_contract(&dest_rpc, config.dest_offramp, "destination offRamp")?;
        ensure_onramp_reachable(&source_rpc, config.source_onramp, "source onRamp")?;
        ensure_offramp_reachable(&source_rpc, config.source_offramp, "source offRamp")?;
        ensure_onramp_reachable(&dest_rpc, config.dest_onramp, "destination onRamp")?;
        ensure_offramp_reachable(&dest_rpc, config.dest_offramp, "destination offRamp")?;
    }

    run_configure_ccv(
        context,
        &source_rpc,
        &private_key,
        &deployer_address,
        config.source_verifier,
        selectors.destination,
        config.source_router,
    )?;
    run_configure_ccv(
        context,
        &dest_rpc,
        &private_key,
        &deployer_address,
        config.dest_verifier,
        selectors.source,
        config.dest_router,
    )?;

    Ok(())
}

fn ensure_has_code(rpc_url: &str, address: alloy::primitives::Address, label: &str) -> Result<()> {
    if AlloyEth.has_code(rpc_url, address)? {
        Ok(())
    } else {
        bail!("{label} has no code at {address}");
    }
}

#[derive(Debug, Clone, Copy)]
struct ChainSelectors {
    source: u64,
    destination: u64,
}

#[derive(Debug, Clone, Copy)]
struct ConfigureInputs {
    source_verifier: alloy::primitives::Address,
    dest_verifier: alloy::primitives::Address,
    source_router: alloy::primitives::Address,
    dest_router: alloy::primitives::Address,
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
        source: if let Some(selector) = runtime::setting(context, "CCV_SOURCE_CHAIN_SELECTOR") {
            selector.parse()?
        } else {
            env_config.ccip_selector(ChainRole::Source)?
        },
        destination: if let Some(selector) = runtime::setting(context, "CCV_DEST_CHAIN_SELECTOR") {
            selector.parse()?
        } else {
            env_config.ccip_selector(ChainRole::Destination)?
        },
    })
}

fn ccv_storage_location_uris(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<String> {
    if let Some(value) = runtime::setting(context, "CCV_STORAGE_LOCATION_URIS")
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(value);
    }
    if env_config.is_local() {
        return Ok(LOCAL_CCV_STORAGE_LOCATION_URIS.to_string());
    }
    bail!("CCV_STORAGE_LOCATION_URIS is required")
}

fn configure_inputs(
    context: &ResolvedContext,
    deployments: &DeploymentsConfig,
) -> Result<ConfigureInputs> {
    Ok(ConfigureInputs {
        source_verifier: resolve_address(
            context,
            "CCV_SOURCE_VERIFIER_ADDRESS",
            deployments.deployment(ChainRole::Source, "chainlinkCcv.verifier"),
            "source SymbioticVerifier",
        )?,
        dest_verifier: resolve_address(
            context,
            "CCV_DEST_VERIFIER_ADDRESS",
            deployments.deployment(ChainRole::Destination, "chainlinkCcv.verifier"),
            "destination SymbioticVerifier",
        )?,
        source_router: resolve_address(
            context,
            "CCV_SOURCE_ROUTER_ADDRESS",
            deployments.deployment(ChainRole::Source, "chainlinkCcv.router"),
            "source router",
        )?,
        dest_router: resolve_address(
            context,
            "CCV_DEST_ROUTER_ADDRESS",
            deployments.deployment(ChainRole::Destination, "chainlinkCcv.router"),
            "destination router",
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
            "script/symbiotic/DeployRelayInfra.s.sol:DeployRelayInfra".to_string(),
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

/// Shared authentication/factory bundle threaded through the per-chain CCV
/// deploy calls. `rpc_url` is the one field that legitimately differs between
/// the source and destination invocations; construct one session per chain
/// (e.g. via struct-update syntax overriding just `rpc_url`).
#[derive(Debug, Clone, Copy)]
struct CcvDeploySession<'a> {
    rpc_url: &'a str,
    private_key: &'a str,
    deployer_address: &'a str,
    factory_private_key: &'a str,
    factory_deployer_address: &'a str,
    storage_location_uris: &'a str,
}

fn run_deploy_ccv(
    context: &ResolvedContext,
    source_session: &CcvDeploySession,
    dest_session: &CcvDeploySession,
    source_settlement: &str,
    dest_settlement: &str,
    selectors: &ChainSelectors,
) -> Result<()> {
    run_deploy_ccv_chain(
        context,
        ChainRole::Source,
        source_session,
        source_settlement,
        selectors.destination,
    )?;
    run_deploy_ccv_chain(
        context,
        ChainRole::Destination,
        dest_session,
        dest_settlement,
        selectors.source,
    )
}

fn run_deploy_ccv_chain(
    context: &ResolvedContext,
    role: ChainRole,
    session: &CcvDeploySession,
    settlement: &str,
    remote_selector: u64,
) -> Result<()> {
    let deployment_role = role_label(role);
    let common_envs = vec![
        (
            "DEPLOYER_ADDRESS".to_string(),
            session.deployer_address.to_string(),
        ),
        (
            "CCV_FACTORY_DEPLOYER".to_string(),
            session.factory_deployer_address.to_string(),
        ),
        (
            "CCV_RESOLVER_OWNER".to_string(),
            session.deployer_address.to_string(),
        ),
        (
            "CCV_DEPLOYMENT_ROLE".to_string(),
            deployment_role.to_string(),
        ),
        (
            "CCV_STORAGE_LOCATION_URIS".to_string(),
            session.storage_location_uris.to_string(),
        ),
    ];

    run_ccv_script(
        context,
        session.rpc_url,
        session.factory_private_key,
        "deployFactory(address[])",
        &[format!("[{}]", session.deployer_address)],
        &common_envs,
    )?;
    let factory = read_address(
        &chainlink_deploy_data_dir(context).join("ccv_factory.json"),
        "factory",
    )?;

    run_ccv_script(
        context,
        session.rpc_url,
        session.private_key,
        "deployResolver(address)",
        &[session.deployer_address.to_string()],
        &common_envs,
    )?;
    let resolver = read_address(
        &chainlink_deploy_data_dir(context).join("ccv_resolver.json"),
        "resolver",
    )?;

    run_ccv_script(
        context,
        session.rpc_url,
        session.private_key,
        "deployLocalMocks(uint64)",
        &[remote_selector.to_string()],
        &common_envs,
    )?;
    let deployment_path = match role {
        ChainRole::Source => source_ccv_contracts_path(context),
        ChainRole::Destination => dest_ccv_contracts_path(context),
    };
    let rmn = read_address(&deployment_path, "rmn")?;

    run_ccv_script(
        context,
        session.rpc_url,
        session.private_key,
        "deployVerifier(address,address,bytes4)",
        &[settlement.to_string(), rmn, CCV_VERSION_TAG.to_string()],
        &common_envs,
    )?;
    let verifier = read_address(&deployment_path, "verifier")?;

    run_ccv_script(
        context,
        session.rpc_url,
        session.private_key,
        "registerVerifier(address,bytes4,address,uint64[])",
        &[
            resolver,
            CCV_VERSION_TAG.to_string(),
            verifier,
            format!("[{remote_selector}]"),
        ],
        &common_envs,
    )?;

    ui::detail(
        &format!("{deployment_role} CCV"),
        format!("factory {factory}"),
    );
    Ok(())
}

/// Local CCV deploys use a plain-CREATE factory deployer key that must be at
/// nonce 0 (`DeployCCV.s.sol`'s `deployFactory` reverts with `require(nonce
/// == 0)` otherwise, since the factory's address is fully determined by that
/// key alone). Anvil state persists across `docker compose` restarts via the
/// `data/anvil-*` bind mounts, so a resumed local chain can carry a stale
/// nonce that would otherwise die deep inside the forge script with an
/// opaque revert. Check up front and fail fast with an actionable message.
fn ensure_factory_deployer_fresh<E: EthApi>(
    eth: &E,
    source_rpc: &str,
    factory_deployer_address: alloy::primitives::Address,
) -> Result<()> {
    let nonce = eth.nonce(source_rpc, factory_deployer_address)?;
    check_factory_deployer_nonce(nonce, factory_deployer_address)
}

/// Pure decision extracted from [`ensure_factory_deployer_fresh`] for unit
/// testing without an `EthApi`.
fn check_factory_deployer_nonce(
    nonce: u64,
    factory_deployer_address: alloy::primitives::Address,
) -> Result<()> {
    if nonce != 0 {
        bail!(
            "local chains carry stale state (factory deployer {factory_deployer_address} is at \
             nonce {nonce}; the CCV factory requires nonce 0). Run 'make chains FRESH=1' to \
             reset the local chains, then re-run 'make deploy'."
        );
    }
    Ok(())
}

fn run_ccv_script(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    signature: &str,
    signature_args: &[String],
    envs: &[(String, String)],
) -> Result<()> {
    let mut args = vec![
        "script".to_string(),
        "script/chainlink/DeployCCV.s.sol:DeployCCV".to_string(),
        "--sig".to_string(),
        signature.to_string(),
    ];
    args.extend(signature_args.iter().cloned());
    args.extend([
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        "--quiet".to_string(),
    ]);
    run_forge(context, &args, envs)
}

fn run_configure_ccv(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
    verifier: alloy::primitives::Address,
    remote_selector: u64,
    router: alloy::primitives::Address,
) -> Result<()> {
    let mut envs = vec![
        ("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string()),
        (
            "CCV_REMOTE_CHAIN_SELECTOR".to_string(),
            remote_selector.to_string(),
        ),
        ("CCV_ROUTER_ADDRESS".to_string(), router.to_string()),
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
        "script/chainlink/ConfigureCCV.s.sol:ConfigureCCV".to_string(),
        "--sig".to_string(),
        "run(address)".to_string(),
        verifier.to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &args, &envs)
}

// ─────────────────────────── Real-CCIP helpers ───────────────────────────

#[derive(Debug, Clone)]
struct ChainlinkCcipPredeploys {
    router: String,
    rmn: String,
    on_ramp: String,
    off_ramp: String,
}

fn uses_real_ccip(env_config: &EnvironmentConfig) -> bool {
    env_config
        .predeploy(ChainRole::Source, "chainlinkCcip", "router")
        .is_some()
        || env_config
            .predeploy(ChainRole::Destination, "chainlinkCcip", "router")
            .is_some()
}

fn has_symbiotic_core(env_config: &EnvironmentConfig, role: ChainRole) -> bool {
    env_config
        .predeploy(role, "symbioticCore", "vaultFactory")
        .is_some()
}

fn chainlink_ccip_predeploys(
    env_config: &EnvironmentConfig,
    role: ChainRole,
) -> Result<ChainlinkCcipPredeploys> {
    let role_label = match role {
        ChainRole::Source => "source",
        ChainRole::Destination => "destination",
    };
    let router = env_config
        .predeploy(role, "chainlinkCcip", "router")
        .ok_or_else(|| eyre!("missing {role_label} chainlinkCcip.router predeploy"))?;
    let rmn = env_config
        .predeploy(role, "chainlinkCcip", "rmn")
        .ok_or_else(|| eyre!("missing {role_label} chainlinkCcip.rmn predeploy"))?;
    let on_ramp = env_config
        .predeploy(role, "chainlinkCcip", "onRamp")
        .ok_or_else(|| eyre!("missing {role_label} chainlinkCcip.onRamp predeploy"))?;
    let off_ramp = env_config
        .predeploy(role, "chainlinkCcip", "offRamp")
        .ok_or_else(|| eyre!("missing {role_label} chainlinkCcip.offRamp predeploy"))?;
    Ok(ChainlinkCcipPredeploys {
        router,
        rmn,
        on_ramp,
        off_ramp,
    })
}

fn run_deploy_noop_settlement(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
) -> Result<String> {
    if let Some(addr) = deployed_address(&noop_settlement_path(context), "settlement", rpc_url)? {
        return Ok(addr);
    }
    let envs = vec![("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string())];
    let args = vec![
        "script".to_string(),
        "script/chainlink/DeployCCV.s.sol:DeployCCV".to_string(),
        "--sig".to_string(),
        "deployNoOpSettlement()".to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &args, &envs)?;
    read_address(&noop_settlement_path(context), "settlement")
}

/// Per-chain real-CCIP-specific inputs to the resolver/verifier-only deploy
/// flow — what varies between the source and destination calls, alongside a
/// [`CcvDeploySession`].
struct CcvOnlyChainInputs<'a> {
    settlement: &'a str,
    ccip: &'a ChainlinkCcipPredeploys,
    remote_selector: u64,
}

fn run_deploy_ccv_only(
    context: &ResolvedContext,
    source_session: &CcvDeploySession,
    dest_session: &CcvDeploySession,
    source_target: &CcvOnlyChainInputs,
    dest_target: &CcvOnlyChainInputs,
) -> Result<()> {
    run_deploy_ccv_only_chain(context, ChainRole::Source, source_session, source_target)?;
    run_deploy_ccv_only_chain(context, ChainRole::Destination, dest_session, dest_target)
}

fn run_deploy_ccv_only_chain(
    context: &ResolvedContext,
    role: ChainRole,
    session: &CcvDeploySession,
    target: &CcvOnlyChainInputs,
) -> Result<()> {
    let rpc_url = session.rpc_url;
    let private_key = session.private_key;
    let deployer_address = session.deployer_address;
    let factory_private_key = session.factory_private_key;
    let factory_deployer_address = session.factory_deployer_address;
    let storage_location_uris = session.storage_location_uris;
    let settlement = target.settlement;
    let ccip = target.ccip;
    let remote_selector = target.remote_selector;

    let deployment_role = role_label(role);
    let deployment_path = match role {
        ChainRole::Source => source_ccv_contracts_path(context),
        ChainRole::Destination => dest_ccv_contracts_path(context),
    };

    if let Some(addr) = deployed_address(&deployment_path, "verifier", rpc_url)?
        && artifact_field_eq(&deployment_path, "settlement", settlement)
        && artifact_field_eq(&deployment_path, "router", &ccip.router)
        && artifact_field_eq(&deployment_path, "rmn", &ccip.rmn)
        && artifact_field_eq(&deployment_path, "onRamp", &ccip.on_ramp)
        && artifact_field_eq(&deployment_path, "offRamp", &ccip.off_ramp)
    {
        ui::info(&format!(
            "{deployment_role} SymbioticVerifier already deployed at {addr}; skipping"
        ));
        return Ok(());
    }

    let common_envs = vec![
        ("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string()),
        (
            "CCV_FACTORY_DEPLOYER".to_string(),
            factory_deployer_address.to_string(),
        ),
        (
            "CCV_RESOLVER_OWNER".to_string(),
            deployer_address.to_string(),
        ),
        (
            "CCV_STORAGE_LOCATION_URIS".to_string(),
            storage_location_uris.to_string(),
        ),
        (
            "CCV_REMOTE_CHAIN_SELECTOR".to_string(),
            remote_selector.to_string(),
        ),
    ];

    // The reserved factory deployer key must be at nonce 0, so the factory's
    // address is fully determined by that key alone. Derive it and check
    // on-chain code directly, rather than trusting local artifact files that
    // may have been wiped (e.g. a deleted deploy-data dir) — otherwise a
    // redeploy is attempted and reverts (nonce != 0 / CREATE2 collision).
    let factory_path = chainlink_deploy_data_dir(context).join("ccv_factory.json");
    let expected_factory = expected_factory_address(factory_deployer_address)?;
    if AlloyEth.has_code(rpc_url, expected_factory)? {
        ensure_artifact_agrees(&factory_path, "factory", expected_factory, "CCV CREATE2 factory")?;
        let chain_id = AlloyEth.chain_id(rpc_url)?;
        write_factory_artifact(&factory_path, chain_id, factory_deployer_address, expected_factory)?;
        ui::info(&format!(
            "{deployment_role} CREATE2Factory already deployed at {expected_factory}; skipping"
        ));
    } else {
        run_ccv_script(
            context,
            rpc_url,
            factory_private_key,
            "deployFactory(address[])",
            &[format!("[{deployer_address}]")],
            &common_envs,
        )?;
    }

    // CREATE2 pins the resolver to the same address on every chain; derive
    // its expected address from the (possibly just-skipped) factory and check
    // on-chain code directly, for the same resumability reason as above.
    let resolver_path = chainlink_deploy_data_dir(context).join("ccv_resolver.json");
    let expected_resolver = expected_resolver_address(context, expected_factory)?;
    if AlloyEth.has_code(rpc_url, expected_resolver)? {
        ensure_artifact_agrees(&resolver_path, "resolver", expected_resolver, "CCV resolver")?;
        let chain_id = AlloyEth.chain_id(rpc_url)?;
        write_resolver_artifact(
            &resolver_path,
            chain_id,
            expected_factory,
            deployer_address,
            expected_resolver,
        )?;
        ui::info(&format!(
            "{deployment_role} VersionedVerifierResolver already deployed at {expected_resolver}; skipping"
        ));
    } else {
        run_ccv_script(
            context,
            rpc_url,
            private_key,
            "deployResolver(address)",
            &[deployer_address.to_string()],
            &common_envs,
        )?;
    }

    let signature = match role {
        ChainRole::Source => "deploySourceCcvOnly(address,address,address,address,address)",
        ChainRole::Destination => "deployDestCcvOnly(address,address,address,address,address)",
    };
    run_ccv_script(
        context,
        rpc_url,
        private_key,
        signature,
        &[
            settlement.to_string(),
            ccip.router.clone(),
            ccip.rmn.clone(),
            ccip.on_ramp.clone(),
            ccip.off_ramp.clone(),
        ],
        &common_envs,
    )
}

fn run_deploy_noop_executor(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
) -> Result<String> {
    if let Some(addr) = deployed_address(&noop_executor_path(context), "executor", rpc_url)? {
        return Ok(addr);
    }
    let envs = vec![("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string())];
    let args = vec![
        "script".to_string(),
        "script/chainlink/DeployExampleCcipApp.s.sol:DeployExampleCcipApp".to_string(),
        "--sig".to_string(),
        "deployExecutor()".to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &args, &envs)?;
    read_address(&noop_executor_path(context), "executor")
}

#[allow(clippy::too_many_arguments)]
fn run_deploy_example_app(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
    router: &str,
    ccv: &str,
    executor: &str,
    output_path: &str,
) -> Result<String> {
    let artifact = context.project_root.join("contracts").join(output_path);
    if let Some(addr) = deployed_address(&artifact, "app", rpc_url)?
        && artifact_field_eq(&artifact, "router", router)
        && artifact_field_eq(&artifact, "ccv", ccv)
        && artifact_field_eq(&artifact, "executor", executor)
    {
        ui::info(&format!(
            "ExampleCcipApp already deployed at {addr} ({output_path}); skipping"
        ));
        return Ok(addr);
    }
    let envs = vec![("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string())];
    let args = vec![
        "script".to_string(),
        "script/chainlink/DeployExampleCcipApp.s.sol:DeployExampleCcipApp".to_string(),
        "--sig".to_string(),
        "deployApp(address,address,address,string)".to_string(),
        router.to_string(),
        ccv.to_string(),
        executor.to_string(),
        output_path.to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        // No --quiet here on purpose: ExampleCcipApp deploys can fail with
        // forge's terse "script failed: <empty revert data>" message which
        // hides the real cause (e.g. missing DEPLOYER_ADDRESS env). Keep
        // stderr visible so the underlying error reaches command_failure().
    ];
    run_forge(context, &args, &envs)?;
    read_address(&PathBuf::from("contracts").join(output_path), "app").or_else(|_| {
        // run_forge cd's into contracts/, so the output path is relative to contracts/.
        read_address(
            &context.project_root.join("contracts").join(output_path),
            "app",
        )
    })
}

fn run_set_remote_app(
    context: &ResolvedContext,
    rpc_url: &str,
    private_key: &str,
    deployer_address: &str,
    app: &str,
    remote_selector: u64,
    remote_app: &str,
) -> Result<()> {
    let envs = vec![("DEPLOYER_ADDRESS".to_string(), deployer_address.to_string())];
    let args = vec![
        "script".to_string(),
        "script/chainlink/DeployExampleCcipApp.s.sol:DeployExampleCcipApp".to_string(),
        "--sig".to_string(),
        "setRemote(address,uint64,address)".to_string(),
        app.to_string(),
        remote_selector.to_string(),
        remote_app.to_string(),
        "--rpc-url".to_string(),
        rpc_url.to_string(),
        "--broadcast".to_string(),
        "--private-key".to_string(),
        private_key.to_string(),
        "--non-interactive".to_string(),
        "--quiet".to_string(),
    ];
    run_forge(context, &args, &envs)
}

fn noop_settlement_path(context: &ResolvedContext) -> PathBuf {
    chainlink_deploy_data_dir(context).join("noop_settlement.json")
}

fn noop_executor_path(context: &ResolvedContext) -> PathBuf {
    chainlink_deploy_data_dir(context).join("noop_executor.json")
}

fn source_ccv_contracts_path(context: &ResolvedContext) -> PathBuf {
    chainlink_deploy_data_dir(context).join("ccv_source_contracts.json")
}

fn dest_ccv_contracts_path(context: &ResolvedContext) -> PathBuf {
    chainlink_deploy_data_dir(context).join("ccv_dest_contracts.json")
}

fn read_address(path: &Path, key: &str) -> Result<String> {
    let json = read_json_value(path)?;
    json.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| eyre!("missing {key} in {}", path.display()))
}

/// True if the artifact at `path` has `key` set to `expected` (case-insensitive
/// hex compare). Used to detect stale resumable deploy artifacts whose recorded
/// dependencies point at older contracts.
fn artifact_field_eq(path: &Path, key: &str, expected: &str) -> bool {
    let Ok(json) = read_json_value(path) else {
        return false;
    };
    json.get(key)
        .and_then(Value::as_str)
        .map(|s| s.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

/// Returns the deployed address if the artifact at `path` exists, contains
/// `key`, and the address has bytecode on `rpc_url`. Otherwise returns `None`.
/// Used to make `deploy_real_ccip` resumable: forge steps whose artifacts are
/// already on-chain are skipped on retry.
fn deployed_address(path: &Path, key: &str, rpc_url: &str) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let Ok(json) = read_json_value(path) else {
        return Ok(None);
    };
    let Some(addr) = json.get(key).and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(parsed) = parse_address(addr) else {
        return Ok(None);
    };
    if AlloyEth.has_code(rpc_url, parsed)? {
        Ok(Some(addr.to_owned()))
    } else {
        Ok(None)
    }
}

fn resolver_salt() -> [u8; 32] {
    alloy::primitives::keccak256(RESOLVER_SALT_PREIMAGE).0
}

/// Expected CREATE2Factory address. The factory is deployed with a plain
/// CREATE from the reserved factory-deployer key, which must be at nonce 0,
/// so its address is fully determined by the deployer address alone —
/// independent of any local artifact file.
fn expected_factory_address(factory_deployer_address: &str) -> Result<alloy::primitives::Address> {
    let deployer = parse_address(factory_deployer_address)
        .ok_or_else(|| eyre!("invalid factory deployer address: {factory_deployer_address}"))?;
    Ok(deployer.create(0))
}

/// Expected `VersionedVerifierResolver` address: CREATE2'd by `factory` using
/// the fixed `RESOLVER_SALT` and the resolver's published creation bytecode
/// (mirrors `DeployCCV.s.sol`'s `deployResolver` / `_resolverCreationCode`).
fn expected_resolver_address(
    context: &ResolvedContext,
    factory: alloy::primitives::Address,
) -> Result<alloy::primitives::Address> {
    let bytecode_path = context
        .project_root
        .join("contracts")
        .join(RESOLVER_BYTECODE_PATH);
    let raw = fs::read_to_string(&bytecode_path)
        .map_err(|err| eyre!("failed to read {}: {err}", bytecode_path.display()))?;
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let init_code = alloy::hex::decode(hex)
        .map_err(|err| eyre!("failed to decode {}: {err}", bytecode_path.display()))?;
    Ok(factory.create2_from_code(resolver_salt(), init_code))
}

/// If the artifact at `path` already records an address for `key` that
/// disagrees with `expected`, fail loudly instead of silently overwriting it
/// — that would indicate a config/keystore mismatch (e.g. the wrong
/// factory-deployer key configured for this chain).
fn ensure_artifact_agrees(
    path: &Path,
    key: &str,
    expected: alloy::primitives::Address,
    label: &str,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let Ok(json) = read_json_value(path) else {
        return Ok(());
    };
    let Some(recorded) = json.get(key).and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(recorded_addr) = parse_address(recorded) else {
        return Ok(());
    };
    if recorded_addr != expected {
        bail!(
            "{label} on-chain address {expected} disagrees with {} recorded in {} ({recorded}); \
             refusing to overwrite — check for a config/keystore mismatch",
            key,
            path.display()
        );
    }
    Ok(())
}

/// Writes `deploy-data/ccv_factory.json` in the same shape as
/// `DeployCCV.s.sol`'s `_saveFactory`, so a subsequent `deployResolver` forge
/// invocation (which reads `.factory` from this file) — or a later resumed
/// run — sees a consistent artifact even if `deploy-data/` was wiped.
fn write_factory_artifact(
    path: &Path,
    chain_id: u64,
    deployer: &str,
    factory: alloy::primitives::Address,
) -> Result<()> {
    write_pretty_json(
        path,
        &json!({
            "chainId": chain_id,
            "deployer": deployer,
            "factory": factory.to_string(),
        }),
    )
}

/// Writes `deploy-data/ccv_resolver.json` in the same shape as
/// `DeployCCV.s.sol`'s `_saveResolver`.
fn write_resolver_artifact(
    path: &Path,
    chain_id: u64,
    factory: alloy::primitives::Address,
    resolver_owner: &str,
    resolver: alloy::primitives::Address,
) -> Result<()> {
    write_pretty_json(
        path,
        &json!({
            "chainId": chain_id,
            "factory": factory.to_string(),
            "resolverOwner": resolver_owner,
            "salt": format!("0x{}", alloy::hex::encode(resolver_salt())),
            "resolver": resolver.to_string(),
        }),
    )
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
    read_address(path, "settlement")
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

fn symbiotic_core_config(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    role: ChainRole,
) -> Result<Option<String>> {
    let chain = env_config.chain(role);
    if chain.predeploys.get("symbioticCore").is_none() && !deployments.role_has_entries(role) {
        return Ok(None);
    }
    // Write inside contracts/.tmp/ so forge's fs_permissions allow reading it.
    let tmp_dir = std::path::Path::new("contracts").join(".tmp");
    fs::create_dir_all(&tmp_dir)?;
    let temp = NamedTempFile::new_in(&tmp_dir)?;
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
    // forge runs with cwd = contracts/, so make path relative to that.
    let path = path
        .strip_prefix("contracts")
        .map(|p| p.to_path_buf())
        .unwrap_or(path);
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
    symbiotic_deploy_data_dir(context).join("relay_infra_source.json")
}

fn dest_relay_infra_path(context: &ResolvedContext) -> PathBuf {
    symbiotic_deploy_data_dir(context).join("relay_infra.json")
}

fn contracts_deploy_data_dir(context: &ResolvedContext) -> PathBuf {
    context.deploy_data_dir()
}

/// Provider-scoped subdirectory for Chainlink CCV deploy artifacts, so stale
/// artifacts from another provider can never leak into this provider's
/// publish/detection flow.
fn chainlink_deploy_data_dir(context: &ResolvedContext) -> PathBuf {
    contracts_deploy_data_dir(context).join("chainlink")
}

/// Provider-scoped subdirectory for Symbiotic relay infrastructure artifacts,
/// shared by both the Chainlink CCV and LayerZero deploy flows.
fn symbiotic_deploy_data_dir(context: &ResolvedContext) -> PathBuf {
    contracts_deploy_data_dir(context).join("symbiotic")
}

fn checkpoint_deployment_state(context: &ResolvedContext) -> Result<()> {
    publish::publish(context, crate::config::Provider::ChainlinkCcv)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    // https://ethereum.stackexchange.com/questions/760/how-is-the-address-of-an-ethereum-contract-computed
    #[test]
    fn expected_factory_address_matches_known_create_vector() {
        let expected = expected_factory_address("0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0").unwrap();
        assert_eq!(
            expected,
            "0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d"
                .parse::<alloy::primitives::Address>()
                .unwrap()
        );
    }

    #[test]
    fn expected_factory_address_rejects_invalid_input() {
        assert!(expected_factory_address("not-an-address").is_err());
    }

    #[test]
    fn check_factory_deployer_nonce_passes_at_zero() {
        let addr = "0x1111111111111111111111111111111111111111"
            .parse::<alloy::primitives::Address>()
            .unwrap();
        assert!(check_factory_deployer_nonce(0, addr).is_ok());
    }

    #[test]
    fn check_factory_deployer_nonce_bails_with_actionable_message_when_stale() {
        let addr = "0x1111111111111111111111111111111111111111"
            .parse::<alloy::primitives::Address>()
            .unwrap();
        let err = check_factory_deployer_nonce(1, addr).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("stale state"));
        assert!(message.contains("nonce 1"));
        assert!(message.contains("make chains FRESH=1"));
        assert!(message.contains("make deploy"));
    }

    // https://eips.ethereum.org/EIPS/eip-1014
    #[test]
    fn expected_resolver_address_matches_known_create2_vector() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let bytecode_path = root.join("contracts").join(RESOLVER_BYTECODE_PATH);
        fs::create_dir_all(bytecode_path.parent().unwrap()).unwrap();
        // Deliberately include the "0x" prefix and surrounding whitespace, to
        // exercise the same trim/strip handling as the real published file.
        fs::write(&bytecode_path, "  0xdeadbeef\n").unwrap();

        let context = ResolvedContext {
            project_root: root,
            env_name: "test".to_string(),
            env_config: PathBuf::from("env.json"),
            deployments: PathBuf::from("deployments.json"),
            generated_dir: PathBuf::from("generated"),
        };
        let factory = "0x00000000000000000000000000000000deadbeef"
            .parse::<alloy::primitives::Address>()
            .unwrap();

        let actual = expected_resolver_address(&context, factory).unwrap();

        let expected = factory.create2_from_code(resolver_salt(), alloy::hex::decode("deadbeef").unwrap());
        assert_eq!(actual, expected);
    }

    #[test]
    fn ensure_artifact_agrees_passes_when_missing_or_matching() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("missing.json");
        let addr = "0x1111111111111111111111111111111111111111"
            .parse::<alloy::primitives::Address>()
            .unwrap();

        // Missing file: nothing to disagree with.
        assert!(ensure_artifact_agrees(&path, "factory", addr, "test").is_ok());

        // Matching recorded value: passes.
        write_pretty_json(&path, &json!({ "factory": addr.to_string() })).unwrap();
        assert!(ensure_artifact_agrees(&path, "factory", addr, "test").is_ok());
    }

    #[test]
    fn ensure_artifact_agrees_fails_on_mismatch() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("factory.json");
        let recorded = "0x1111111111111111111111111111111111111111"
            .parse::<alloy::primitives::Address>()
            .unwrap();
        let expected = "0x2222222222222222222222222222222222222222"
            .parse::<alloy::primitives::Address>()
            .unwrap();
        write_pretty_json(&path, &json!({ "factory": recorded.to_string() })).unwrap();

        let err = ensure_artifact_agrees(&path, "factory", expected, "CCV CREATE2 factory")
            .unwrap_err();
        assert!(err.to_string().contains("disagrees"));
    }

    #[test]
    fn write_factory_artifact_matches_deploy_ccv_script_shape() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("ccv_factory.json");
        let factory = "0x2222222222222222222222222222222222222222"
            .parse::<alloy::primitives::Address>()
            .unwrap();

        write_factory_artifact(&path, 1337, "0x1111111111111111111111111111111111111111", factory)
            .unwrap();

        let json = read_json_value(&path).unwrap();
        assert_eq!(json["chainId"], 1337);
        assert_eq!(
            json["deployer"],
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(json["factory"], factory.to_string());
    }

    #[test]
    fn write_resolver_artifact_matches_deploy_ccv_script_shape() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("ccv_resolver.json");
        let factory = "0x1111111111111111111111111111111111111111"
            .parse::<alloy::primitives::Address>()
            .unwrap();
        let resolver = "0x2222222222222222222222222222222222222222"
            .parse::<alloy::primitives::Address>()
            .unwrap();

        write_resolver_artifact(
            &path,
            1337,
            factory,
            "0x3333333333333333333333333333333333333333",
            resolver,
        )
        .unwrap();

        let json = read_json_value(&path).unwrap();
        assert_eq!(json["chainId"], 1337);
        assert_eq!(json["factory"], factory.to_string());
        assert_eq!(
            json["resolverOwner"],
            "0x3333333333333333333333333333333333333333"
        );
        assert_eq!(
            json["salt"],
            format!("0x{}", alloy::hex::encode(resolver_salt()))
        );
        assert_eq!(json["resolver"], resolver.to_string());
    }
}
