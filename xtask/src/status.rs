use colored::Colorize;
use eyre::Result;

use crate::config::{DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi};
use crate::runner::{CommandRunner, CommandSpec, SystemRunner};
use crate::runtime;
use crate::ui;

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = DeploymentsConfig::load(&context.deployments).ok();
    let runner = SystemRunner;
    let eth = AlloyEth;

    ui::header(
        "status",
        &context.env_name,
        Some(env_config.active_provider.as_str()),
    );
    ui::section("prerequisites");
    print_prerequisites(context);
    ui::section("container status");
    print_container_status(&runner)?;
    ui::section("health checks");
    print_health_checks(&runner, context)?;
    ui::section("monitor status");
    print_monitor_status(context, &env_config, &eth)?;
    ui::section("deployment status");
    print_deployment_status(&env_config, deployments.as_ref(), &context.env_name);
    Ok(())
}

fn print_prerequisites(context: &ResolvedContext) {
    let env_file = crate::envfile::env_file_path(&context.project_root, &context.env_name);
    let env_ok = env_file.exists();
    let label = format!(".env.{}:", context.env_name);
    ui::field(&label, if env_ok { "OK".green() } else { "MISSING".red() });

    for i in 1..=3 {
        let path = context
            .project_root
            .join("config")
            .join("keys")
            .join(format!("signer-{i}.json"));
        let ok = path.exists();
        ui::field(
            &format!("signer-{i}:"),
            if ok {
                "OK".green()
            } else {
                format!("MISSING — run `cargo xtask generate-signer --name signer-{i}`").red()
            },
        );
    }
}

fn print_container_status<R: CommandRunner>(runner: &R) -> Result<()> {
    let output = runner.run(&CommandSpec::new(
        "docker",
        vec![
            "ps".to_string(),
            "--format".to_string(),
            "table {{.Names}}\t{{.Status}}\t{{.Ports}}".to_string(),
        ],
    ))?;
    let wanted = [
        "NAMES",
        "anvil",
        "operator-",
        "oz-monitor",
        "oz-relayer",
        "redis",
        "symbiotic-relay",
    ];
    let mut printed = false;
    for line in output.stdout.lines() {
        if wanted.iter().any(|needle| line.contains(needle)) {
            println!("{line}");
            printed = true;
        }
    }
    if !printed {
        println!("No containers running");
    }
    Ok(())
}

fn print_health_checks<R: CommandRunner>(runner: &R, context: &ResolvedContext) -> Result<()> {
    for (label, args) in health_commands(context) {
        let ok = runner
            .run(&CommandSpec::new("sh", vec!["-lc".to_string(), args]))
            .map(|output| output.success)
            .unwrap_or(false);
        ui::field(&label, if ok { "OK".green() } else { "NOT RUNNING".red() });
    }
    Ok(())
}

fn print_deployment_status(
    env_config: &EnvironmentConfig,
    deployments: Option<&DeploymentsConfig>,
    env_name: &str,
) {
    match deployments {
        Some(deployments)
            if deployments.role_has_entries(crate::config::ChainRole::Source)
                && deployments.role_has_entries(crate::config::ChainRole::Destination) =>
        {
            println!(
                "Contracts: {}",
                format!("DEPLOYED ({})", env_config.active_provider).green()
            );
            println!("  Source:");
            print_role_summary(deployments, crate::config::ChainRole::Source);
            println!("  Destination:");
            print_role_summary(deployments, crate::config::ChainRole::Destination);
        }
        _ => {
            println!(
                "Contracts: {}",
                format!(
                    "NOT DEPLOYED for '{}' (run 'make deploy ENV={}')",
                    env_config.active_provider, env_name
                )
                .red()
            );
        }
    }
}

fn print_monitor_status<E: EthApi>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    eth: &E,
) -> Result<()> {
    if !env_config.is_local() {
        ui::field("oz-monitor lag:", "n/a (non-local)");
        return Ok(());
    }

    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    let Some(source_rpc) = runtime
        .source_rpc
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        ui::field("oz-monitor lag:", "unknown (missing source RPC)".yellow());
        return Ok(());
    };

    let cursor_file = context
        .project_root
        .join("data")
        .join("oz-monitor")
        .join("local_anvil_last_block.txt");

    let head = eth.block_number(source_rpc).ok();

    let cursor = std::fs::read_to_string(&cursor_file)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());

    match (head, cursor) {
        (Some(head), Some(cursor)) => {
            let lag = head.saturating_sub(cursor.min(head));
            ui::field("oz-monitor lag:", format!("{lag} block(s)"));
        }
        (Some(_), None) => ui::field("oz-monitor lag:", "unknown (cursor missing)".yellow()),
        _ => ui::field("oz-monitor lag:", "unknown (cannot query source head)".yellow()),
    }

    Ok(())
}

fn print_role_summary(deployments: &DeploymentsConfig, role: crate::config::ChainRole) {
    let value = match role {
        crate::config::ChainRole::Source => &deployments.source,
        crate::config::ChainRole::Destination => &deployments.destination,
    };

    if let Some(items) = value.as_object() {
        for (key, value) in items.iter().take(5) {
            println!("    {key}: {value}");
        }
    }
}

fn health_commands(context: &ResolvedContext) -> Vec<(String, String)> {
    let api_key = runtime::setting(context, "OZ_RELAYER_API_KEY");

    let mut commands: Vec<(String, String)> = Vec::new();
    for i in 1..=3 {
        commands.push((
            format!("operator-{i}:"),
            format!("curl -sf http://localhost:{}/healthz >/dev/null", 3000 + i),
        ));
    }

    let relayer_cmd = match api_key {
        Some(api_key) => format!(
            "curl -sf http://localhost:8080/api/v1/health -H 'Authorization: Bearer {api_key}' >/dev/null"
        ),
        None => "false".to_string(),
    };
    commands.push(("oz-relayer:".to_string(), relayer_cmd));
    for i in 1..=3 {
        commands.push((
            format!("symbiotic-relay-{i}:"),
            format!("curl -sf http://localhost:{}/healthz >/dev/null", 8080 + i),
        ));
    }
    commands
}
