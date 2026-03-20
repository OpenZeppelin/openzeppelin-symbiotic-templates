use std::thread;
use std::time::Duration;

use eyre::{Result, bail, eyre};

use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::runner::{CommandRunner, CommandSpec};

const MAX_ATTEMPTS: usize = 3;
const RETRY_DELAY_SECONDS: u64 = 5;
const CONFIG_SERVICE_NAMES: [&str; 4] = ["oz-monitor", "operator-1", "operator-2", "operator-3"];

pub fn ensure_docker_available<R: CommandRunner>(runner: &R) -> Result<()> {
    if command_succeeds(runner, "docker", vec!["info".to_string()]) {
        Ok(())
    } else {
        bail!("Docker daemon is not reachable. Start Docker and retry.");
    }
}

pub fn start<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    refresh_config_services: bool,
    force_recreate_relayer: bool,
) -> Result<()> {
    ensure_docker_available(runner)?;
    start_compose(
        runner,
        context,
        env_config,
        refresh_config_services,
        force_recreate_relayer,
    )
}

pub fn compose_args(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    args.push("-f".to_string());
    args.push(context.project_root.join("docker-compose.yml").display().to_string());
    if env_config.is_local() {
        args.push("-f".to_string());
        args.push(
            context
                .project_root
                .join("docker-compose.local.yml")
                .display()
                .to_string(),
        );
    }
    args
}

fn start_compose<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    refresh_config_services: bool,
    force_recreate_relayer: bool,
) -> Result<()> {
    let mut last_error: Option<String> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        match run_compose_up(runner, context, env_config, refresh_config_services, force_recreate_relayer) {
            Ok(()) => return Ok(()),
            Err(err) => last_error = Some(err.to_string()),
        };

        if attempt < MAX_ATTEMPTS {
            thread::sleep(Duration::from_secs(RETRY_DELAY_SECONDS));
        }
    }

    Err(eyre!(
        "failed to start services: {}",
        last_error.unwrap_or_else(|| "unknown docker compose error".to_string())
    ))
}

fn run_compose_up<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    refresh_config_services: bool,
    force_recreate_relayer: bool,
) -> Result<()> {
    let args = compose_up_args(context, env_config, refresh_config_services, force_recreate_relayer);
    let output = runner.run(&docker_compose_spec(context, args))?;

    if output.success {
        return Ok(());
    }

    Err(eyre!("failed to start services: {}", output.stderr.trim()))
}

fn compose_up_args(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    refresh_config_services: bool,
    force_recreate_relayer: bool,
) -> Vec<String> {
    let mut args = compose_args(context, env_config);
    args.extend([
        "--profile".to_string(),
        "infra".to_string(),
        "--profile".to_string(),
        "dev".to_string(),
        "up".to_string(),
        "-d".to_string(),
        "--remove-orphans".to_string(),
        "--wait".to_string(),
        "--wait-timeout".to_string(),
        "120".to_string(),
    ]);

    let mut services = Vec::new();
    if force_recreate_relayer {
        services.push("oz-relayer".to_string());
    }
    if refresh_config_services {
        services.extend(CONFIG_SERVICE_NAMES.iter().map(|service| service.to_string()));
    }
    if !services.is_empty() {
        args.push("--force-recreate".to_string());
        args.extend(services);
    }

    args
}

fn command_succeeds<R: CommandRunner>(runner: &R, program: &str, args: Vec<String>) -> bool {
    runner
        .run(&CommandSpec::new(program, args))
        .map(|output| output.success)
        .unwrap_or(false)
}

fn docker_compose_spec(context: &ResolvedContext, args: Vec<String>) -> CommandSpec {
    CommandSpec::new("docker", args).with_env("ENV", &context.env_name)
}
