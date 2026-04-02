use std::process::Command;

use eyre::{Result, bail, eyre};

use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi};
use crate::provider;
use crate::publish;
use crate::render;
use crate::runtime;
use crate::ui;
use crate::validate;

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    if !crate::envfile::env_file_exists(&context.project_root, &context.env_name) {
        let path = crate::envfile::env_file_path(&context.project_root, &context.env_name);
        bail!("{} not found.", path.display());
    }
    let env_config = EnvironmentConfig::load(&context.env_config)?;

    if let Ok(existing) = crate::config::DeploymentsConfig::load(&context.deployments) {
        if let Some(deployed_provider) = existing.detected_provider() {
            if deployed_provider != env_config.active_provider {
                bail!(
                    "provider changed ({} -> {}). Run `make clean` first.",
                    deployed_provider,
                    env_config.active_provider
                );
            }
        }
    }

    let eth = AlloyEth;
    let runtime = runtime::RuntimeInputs::resolve(context, &env_config);
    let source_rpc = runtime
        .source_rpc
        .ok_or_else(|| eyre!("SOURCE RPC is not configured"))?;
    let dest_rpc = runtime
        .dest_rpc
        .ok_or_else(|| eyre!("DEST RPC is not configured"))?;

    ui::header(
        "deploy",
        &context.env_name,
        Some(env_config.active_provider.as_str()),
    );

    let build = ui::step("build contracts");
    run_contracts_command(context, &["build", "--quiet"])?;
    build.done("contracts built");

    check_rpc(&eth, &source_rpc, "source chain")?;
    check_rpc(&eth, &dest_rpc, "destination chain")?;

    provider::deploy(context, &env_config)?;

    let publish = ui::step("update deployment state");
    publish::publish(context)?;
    publish.done("deployments updated");

    let artifacts = ui::step("generate service config");
    render::generate_runtime_artifacts(context)?;
    artifacts.done("service config generated");

    let startup = ui::step("prepare service startup");
    provider::configure_startup(context, &env_config)?;
    startup.done("service startup prepared");

    let validate = ui::step("validate deployment");
    validate::validate_or_bail(context, env_config.is_local(), &eth)?;
    validate.done("validation passed");

    ui::ok("deploy complete");

    let next_command = if env_config.is_local() {
        "make start".to_owned()
    } else {
        format!("make start ENV={}", context.env_name)
    };
    ui::next(next_command.as_str());
    Ok(())
}

fn check_rpc<E: EthApi>(eth: &E, rpc_url: &str, name: &str) -> Result<()> {
    if eth.rpc_reachable(rpc_url) {
        return Ok(());
    }
    bail!("{name} not reachable ({rpc_url}). Run `make chains` first.");
}

fn run_contracts_command(context: &ResolvedContext, args: &[&str]) -> Result<()> {
    let mut command = Command::new("forge");
    command
        .current_dir(context.project_root.join("contracts"))
        .args(args);
    let output = ui::run_command(&mut command, "still building contracts")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(ui::command_failure(
            &format!("forge {}", args.join(" ")),
            &output
        )))
    }
}
