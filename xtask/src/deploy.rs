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
    let env_config = EnvironmentConfig::load(&context.env_config)?;
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

    if env_config.is_local() {
        let infra = ui::step("prepare local chains");
        prepare_local_deploy_infra(context)?;
        infra.done("local chains ready");
        wait_for_rpc(&eth, &source_rpc, "source chain")?;
        wait_for_rpc(&eth, &dest_rpc, "settlement chain")?;
    } else {
        wait_for_rpc(&eth, &source_rpc, "source chain")?;
        wait_for_rpc(&eth, &dest_rpc, "destination chain")?;
    }

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
    if env_config.is_local() {
        ui::next("make start");
    } else {
        ui::next(&format!("make run-operators ENV={}", context.env_name));
    }
    Ok(())
}

fn wait_for_rpc<E: EthApi>(eth: &E, rpc_url: &str, name: &str) -> Result<()> {
    let mut step = ui::step(format!("wait for {name}"));
    for _ in 0..30 {
        if eth.rpc_reachable(rpc_url) {
            step.done(&format!("{name} reachable"));
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        step.heartbeat();
    }
    bail!("timeout waiting for {name} ({rpc_url})");
}

fn prepare_local_deploy_infra(context: &ResolvedContext) -> Result<()> {
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
    )
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

fn run_project_command(context: &ResolvedContext, program: &str, args: &[&str]) -> Result<()> {
    let mut command = Command::new(program);
    command
        .current_dir(&context.project_root)
        .args(args)
        .env("ENV", &context.env_name);
    let output = ui::run_command(&mut command, "still preparing local deploy infra")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(ui::command_failure(
            &format!("{program} {}", args.join(" ")),
            &output
        )))
    }
}
