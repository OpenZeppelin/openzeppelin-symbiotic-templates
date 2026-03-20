use eyre::{Result, bail};

use crate::config::{ChainRole, DeploymentsConfig};
use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::deploy;
use crate::eth::AlloyEth;
use crate::operators;
use crate::preflight::{self, PreflightReport};
use crate::runner::SystemRunner;
use crate::render;
use crate::services;
use crate::validate::{self, ValidationReport};

pub fn run_start_local(context: &ResolvedContext) -> Result<()> {
    if context.env_name != "local" {
        bail!("start-local is fixed to env `local`");
    }

    deploy::run_command(context)?;
    prepare_local_runtime(context)?;

    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let runner = SystemRunner;
    services::start(&runner, context, &env_config, false, false)?;

    println!("Local stack started. Run `make status` to check health.");
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

    render::render(context)?;
    maybe_configure_ccv(context, &env_config)?;

    operators::align_relayer_keystores(&runner, context)?;
    ensure_preflight_ok(preflight::preflight(context, &eth))?;
    ensure_validation_ok(validate::validate(context, true, &eth))?;

    println!("Starting services...");
    services::start(&runner, context, &env_config, true, true)?;

    println!(
        "Non-local operator services started. Run `make status ENV={}` to check health.",
        context.env_name
    );
    Ok(())
}

fn ensure_preflight_ok(report: PreflightReport) -> Result<()> {
    if report.failures.is_empty() {
        return Ok(());
    }

    eprintln!("Preflight checks failed:");
    for failure in report.failures {
        eprintln!("  - {failure}");
    }
    bail!("startup preflight failed");
}

fn ensure_validation_ok(report: ValidationReport) -> Result<()> {
    if report.failures.is_empty() {
        return Ok(());
    }

    eprintln!("Validation failed:");
    for failure in report.failures {
        eprintln!("  - {failure}");
    }
    bail!("runtime validation failed");
}

fn maybe_configure_ccv(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    if env_config.active_provider == "chainlink_ccv" {
        crate::bridge::run_make_target(context, "configure-ccv-contracts")?;
    }
    Ok(())
}

fn prepare_local_runtime(context: &ResolvedContext) -> Result<()> {
    println!("Refreshing settlement epoch for local devnet...");
    crate::bridge::run_make_target(context, "refresh-epoch")?;
    println!("Resetting runtime state for deterministic restart...");
    crate::bridge::run_make_target(context, "reset-runtime")?;
    Ok(())
}
