use std::fs;
use std::thread;
use std::time::Duration;

use eyre::{Result, bail, eyre};

use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::envfile;
use crate::runner::{CommandRunner, CommandSpec};

/// Name of the generated file that snapshots the docker-compose
/// interpolation variables (`SOURCE_RPC_URL`, etc.) resolved at `xtask start`
/// time. Raw `docker compose ...` invocations (e.g. the Makefile's
/// `rebuild-operators`/`restart-*` targets) don't go through `xtask` and so
/// don't have these in their process environment; they instead pass
/// `--env-file generated/<env>/compose.env` to pick this snapshot up.
pub const COMPOSE_ENV_FILE_NAME: &str = "compose.env";

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
    write_compose_env_snapshot(context)?;
    start_compose(
        runner,
        context,
        env_config,
        refresh_config_services,
        force_recreate_relayer,
    )?;
    if env_config.is_local() {
        ensure_relay_p2p_connected(runner)?;
    }
    Ok(())
}

/// Snapshot the docker-compose interpolation variables `xtask start` resolves
/// (from `.env.<env>`, with `SOURCE_RPC_URL`/`DEST_RPC_URL` refreshed from any
/// process-env override, matching `envfile::get`'s precedence) into
/// `generated/<env>/compose.env`. This lets Makefile targets that call
/// `docker compose` directly recreate operator/monitor/relayer containers
/// without crash-looping on missing env vars that `xtask` normally supplies
/// via `--env-file .env.<env>` (see `compose_args`).
fn write_compose_env_snapshot(context: &ResolvedContext) -> Result<()> {
    let mut vars = envfile::read_all(&context.project_root, &context.env_name);
    for key in ["SOURCE_RPC_URL", "DEST_RPC_URL"] {
        if let Some(value) = envfile::get(&context.project_root, &context.env_name, key) {
            vars.insert(key.to_string(), value);
        }
    }
    vars.insert("ENV".to_string(), context.env_name.clone());

    let mut lines: Vec<String> = vars
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    lines.sort();

    fs::create_dir_all(&context.generated_dir)?;
    fs::write(
        context.generated_dir.join(COMPOSE_ENV_FILE_NAME),
        format!("{}\n", lines.join("\n")),
    )?;
    Ok(())
}

pub fn start_infra<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
) -> Result<()> {
    ensure_docker_available(runner)?;
    let mut args = compose_args(context, env_config);
    args.extend([
        "--profile".to_string(),
        "infra".to_string(),
        "up".to_string(),
        "-d".to_string(),
        "--remove-orphans".to_string(),
        "--wait".to_string(),
        "--wait-timeout".to_string(),
        "120".to_string(),
    ]);
    let output = runner.run(&docker_compose_spec(context, args))?;
    if output.success {
        Ok(())
    } else {
        Err(eyre!(
            "failed to start infra services: {}",
            output.stderr.trim()
        ))
    }
}

pub fn down<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    remove_volumes: bool,
) -> Result<()> {
    let mut args = compose_args(context, env_config);
    args.extend([
        "--profile".to_string(),
        "dev".to_string(),
        "--profile".to_string(),
        "infra".to_string(),
        "down".to_string(),
    ]);
    if remove_volumes {
        args.push("-v".to_string());
    }
    args.push("--remove-orphans".to_string());

    let output = runner.run(&docker_compose_spec(context, args))?;
    if output.success {
        Ok(())
    } else {
        Err(eyre!("failed to stop services: {}", output.stderr.trim()))
    }
}

/// Container names for the local anvil chains (`docker-compose.local.yml`).
/// These carry on-chain state across restarts via the `data/anvil-*` bind
/// mounts and must never be touched by a service-state reset — only by
/// `xtask chains --fresh`.
pub const ANVIL_CONTAINER_NAMES: [&str; 2] = ["anvil", "anvil-settlement"];

/// Container names for everything else in the `dev` profile
/// (`docker-compose.yml`) — the operator/relay/monitor stack whose runtime
/// state `xtask start --reset` is meant to clear. Deliberately excludes the
/// anvil chain containers above.
pub const SERVICE_STATE_CONTAINER_NAMES: [&str; 9] = [
    "oz-monitor",
    "redis",
    "oz-relayer",
    "symbiotic-relay-1",
    "symbiotic-relay-2",
    "symbiotic-relay-3",
    "operator-1",
    "operator-2",
    "operator-3",
];

/// Stop and remove exactly the named containers (plus, when `remove_volumes`,
/// any named volumes declared for them in the compose files' `volumes:`
/// section — e.g. `redis-data`), regardless of compose profile. `docker
/// compose down <services> --volumes` scopes both container and named-volume
/// removal strictly to the given services (verified: `down anvil --volumes`
/// never touches `redis-data`), so unlike `down` (which downs everything
/// matched by the `dev`/`infra` profiles), this never touches anything
/// outside `services` — used to scope a reset to either service state or
/// chain state without disturbing the other.
pub fn down_services<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    services: &[&str],
    remove_volumes: bool,
) -> Result<()> {
    // Explicit service names are always selected regardless of active
    // profiles, so no --profile flags are needed here (unlike `down`, which
    // operates over the whole project and relies on them).
    let mut args = compose_args(context, env_config);
    args.push("down".to_string());
    args.extend(services.iter().map(|service| service.to_string()));
    if remove_volumes {
        args.push("--volumes".to_string());
    }
    args.push("--remove-orphans".to_string());

    let output = runner.run(&docker_compose_spec(context, args))?;
    if output.success {
        Ok(())
    } else {
        Err(eyre!(
            "failed to stop services {}: {}",
            services.join(", "),
            output.stderr.trim()
        ))
    }
}

pub fn compose_args(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    let env_file = crate::envfile::env_file_path(&context.project_root, &context.env_name);
    if env_file.exists() {
        args.push("--env-file".to_string());
        args.push(env_file.display().to_string());
    }
    args.push("-f".to_string());
    args.push(
        context
            .project_root
            .join("docker-compose.yml")
            .display()
            .to_string(),
    );
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
        match run_compose_up(
            runner,
            context,
            env_config,
            refresh_config_services,
            force_recreate_relayer,
        ) {
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
    let args = compose_up_args(
        context,
        env_config,
        refresh_config_services,
        force_recreate_relayer,
    );
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
        services.extend(
            CONFIG_SERVICE_NAMES
                .iter()
                .map(|service| service.to_string()),
        );
    }
    if !services.is_empty() {
        args.push("--force-recreate".to_string());
        args.extend(services);
    }

    args
}

const RELAY_CONTAINERS: [&str; 3] = [
    "symbiotic-relay-1",
    "symbiotic-relay-2",
    "symbiotic-relay-3",
];
const RELAY_P2P_MAX_RETRIES: usize = 3;
const RELAY_P2P_SETTLE_SECONDS: u64 = 5;

/// Check that all relay containers have established P2P peer connections.
/// If any relay is isolated (0 peers), restart it and recheck. This handles
/// a race condition where mDNS peer discovery fires before the libp2p noise
/// listener is ready, leaving a relay permanently disconnected.
fn ensure_relay_p2p_connected<R: CommandRunner>(runner: &R) -> Result<()> {
    // Give relays a moment to complete P2P handshakes after healthcheck passes
    thread::sleep(Duration::from_secs(RELAY_P2P_SETTLE_SECONDS));

    for attempt in 1..=RELAY_P2P_MAX_RETRIES {
        let isolated: Vec<&str> = RELAY_CONTAINERS
            .iter()
            .filter(|name| !relay_has_peers(runner, name))
            .copied()
            .collect();

        if isolated.is_empty() {
            return Ok(());
        }

        if attempt == RELAY_P2P_MAX_RETRIES {
            bail!(
                "relay P2P connectivity failed after {} attempts: {} still isolated",
                RELAY_P2P_MAX_RETRIES,
                isolated.join(", ")
            );
        }

        for name in &isolated {
            crate::ui::warn(&format!("{name} has no P2P peers, restarting"));
            let _ = runner.run(&CommandSpec::new(
                "docker",
                vec!["restart".to_string(), name.to_string()],
            ));
        }

        // Wait for restarted relay to come up and establish connections
        thread::sleep(Duration::from_secs(RELAY_P2P_SETTLE_SECONDS * 2));
    }

    Ok(())
}

fn relay_has_peers<R: CommandRunner>(runner: &R, container: &str) -> bool {
    runner
        .run(&CommandSpec::new(
            "docker",
            vec![
                "logs".to_string(),
                container.to_string(),
            ],
        ))
        .map(|output| {
            output.stdout.contains("Connected to peer")
                || output.stderr.contains("Connected to peer")
        })
        .unwrap_or(false)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn test_context(root: &std::path::Path, env_name: &str) -> ResolvedContext {
        ResolvedContext {
            project_root: root.to_path_buf(),
            env_name: env_name.to_string(),
            env_config: root.join(format!("config/environments/{env_name}.json")),
            deployments: root.join(format!("deployments/{env_name}.json")),
            generated_dir: root.join("generated").join(env_name),
        }
    }

    #[test]
    fn write_compose_env_snapshot_persists_dotenv_vars() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        // Make sure no leftover process-env override from another test bleeds in.
        unsafe {
            std::env::remove_var("SOURCE_RPC_URL");
            std::env::remove_var("DEST_RPC_URL");
        }

        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::write(
            root.join(".env.local-ccv"),
            "WEBHOOK_SECRET=test-webhook-secret\nSOURCE_RPC_URL=http://anvil:8545\nDEST_RPC_URL=http://anvil-settlement:8546\n",
        )
        .unwrap();
        let context = test_context(&root, "local-ccv");

        write_compose_env_snapshot(&context).unwrap();

        let contents =
            fs::read_to_string(context.generated_dir.join(COMPOSE_ENV_FILE_NAME)).unwrap();
        assert!(contents.contains("WEBHOOK_SECRET=test-webhook-secret"));
        assert!(contents.contains("SOURCE_RPC_URL=http://anvil:8545"));
        assert!(contents.contains("DEST_RPC_URL=http://anvil-settlement:8546"));
        assert!(contents.contains("ENV=local-ccv"));
    }

    #[test]
    fn write_compose_env_snapshot_prefers_process_env_override() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());

        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::write(
            root.join(".env.local-ccv"),
            "SOURCE_RPC_URL=http://anvil:8545\n",
        )
        .unwrap();
        let context = test_context(&root, "local-ccv");

        unsafe {
            std::env::set_var("SOURCE_RPC_URL", "http://override:8545");
        }
        let result = write_compose_env_snapshot(&context);
        unsafe {
            std::env::remove_var("SOURCE_RPC_URL");
        }
        result.unwrap();

        let contents =
            fs::read_to_string(context.generated_dir.join(COMPOSE_ENV_FILE_NAME)).unwrap();
        assert!(contents.contains("SOURCE_RPC_URL=http://override:8545"));
    }
}
