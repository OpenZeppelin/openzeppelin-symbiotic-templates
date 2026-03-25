use std::fs;

use eyre::{Result, bail};

use crate::config::EnvironmentConfig;
use crate::config::{ChainRole, DeploymentsConfig};
use crate::context::ResolvedContext;
use crate::deploy;
use crate::eth::AlloyEth;
use crate::genesis;
use crate::provider;
use crate::render;
use crate::runner::SystemRunner;
use crate::services;
use crate::signers;
use crate::ui;
use crate::validate;

pub fn run_start_local(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    if !env_config.is_local() {
        bail!(
            "start-local requires a local-chain environment; {} is not local",
            context.env_name
        );
    }
    let runner = SystemRunner;
    let eth = AlloyEth;

    ui::header(
        "start-local",
        &context.env_name,
        Some(env_config.active_provider.as_str()),
    );

    if !local_deployment_complete(context) {
        ui::warn("deployment state missing; running deploy first");
        deploy::run_command(context)?;
    }

    let artifacts = ui::step("generate service config");
    render::generate_runtime_artifacts(context)?;
    artifacts.done("service config generated");

    let startup = ui::step("prepare service startup");
    provider::configure_startup(context, &env_config)?;
    startup.done("service startup prepared");

    let signers_step = ui::step("verify relayer signer config");
    signers::verify_signers(context)?;
    signers_step.done("relayer signer config verified");

    let reset = ui::step("reset local runtime state");
    reset_local_runtime(context, &runner, &env_config)?;
    reset.done("local runtime state reset");

    let infra = ui::step("start local infra");
    services::start_infra(&runner, context, &env_config)?;
    infra.done("local infra started");

    let epoch = ui::step("ensure settlement epoch is fresh");
    genesis::ensure_local_epoch_fresh(context, &env_config, &eth)?;
    epoch.done("settlement epoch is fresh");

    let validation = ui::step("validate local stack");
    validate::validate_or_bail(context, true, &eth)?;
    validation.done("local stack validated");

    let services_step = ui::step("start local services");
    services::start(&runner, context, &env_config, false, false)?;
    services_step.done("local services started");

    ui::ok("local stack started");
    ui::next("make status");
    Ok(())
}

pub fn run_run_operators(context: &ResolvedContext) -> Result<()> {
    if context.env_name == "local" {
        bail!("run-operators is non-local only; use `cargo xtask start-local`");
    }

    let env_config = EnvironmentConfig::load(&context.env_config)?;
    if env_config.is_local() {
        bail!("run-operators is non-local only; use `cargo xtask start-local`");
    }

    let deployments = DeploymentsConfig::load(&context.deployments)?;
    if !deployments.role_has_entries(ChainRole::Source)
        || !deployments.role_has_entries(ChainRole::Destination)
    {
        bail!(
            "missing deployment state in {}. Run `make deploy ENV={}` first.",
            context.deployments.display(),
            context.env_name
        );
    }

    let runner = SystemRunner;
    let eth = AlloyEth;

    ui::header(
        "run-operators",
        &context.env_name,
        Some(env_config.active_provider.as_str()),
    );

    let artifacts = ui::step("generate service config");
    render::generate_runtime_artifacts(context)?;
    artifacts.done("service config generated");

    let startup = ui::step("prepare service startup");
    provider::configure_startup(context, &env_config)?;
    startup.done("service startup prepared");

    let signers_step = ui::step("verify relayer signer config");
    signers::verify_signers(context)?;
    signers_step.done("relayer signer config verified");

    let validation = ui::step("validate operator stack");
    validate::validate_or_bail(context, true, &eth)?;
    validation.done("operator stack validated");

    let services_step = ui::step("start operator services");
    services::start(&runner, context, &env_config, true, true)?;
    services_step.done("operator services started");

    ui::ok("operator services started");
    ui::next(&format!("make status ENV={}", context.env_name));
    Ok(())
}

fn local_deployment_complete(context: &ResolvedContext) -> bool {
    DeploymentsConfig::load(&context.deployments)
        .map(|deployments| {
            deployments.role_has_entries(ChainRole::Source)
                && deployments.role_has_entries(ChainRole::Destination)
        })
        .unwrap_or(false)
}

fn reset_local_runtime(
    context: &ResolvedContext,
    runner: &SystemRunner,
    env_config: &EnvironmentConfig,
) -> Result<()> {
    services::down(runner, context, env_config, true)?;
    clear_dir_contents(context.project_root.join("data").join("sidecar-1"))?;
    clear_dir_contents(context.project_root.join("data").join("sidecar-2"))?;
    clear_dir_contents(context.project_root.join("data").join("sidecar-3"))?;
    remove_file_if_exists(
        context
            .project_root
            .join("data")
            .join("oz-monitor")
            .join("local_anvil_last_block.txt"),
    )?;
    Ok(())
}

fn clear_dir_contents(path: impl AsRef<std::path::Path>) -> Result<()> {
    let path = path.as_ref();
    fs::create_dir_all(path)?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            fs::remove_dir_all(child)?;
        } else {
            fs::remove_file(child)?;
        }
    }
    Ok(())
}

fn remove_file_if_exists(path: impl AsRef<std::path::Path>) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
