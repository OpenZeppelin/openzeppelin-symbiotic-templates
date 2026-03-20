use eyre::Result;

use crate::config::{DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi};
use crate::runner::{CommandRunner, CommandSpec, SystemRunner};
use crate::runtime;

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = DeploymentsConfig::load(&context.deployments).ok();
    let runner = SystemRunner;
    let eth = AlloyEth;

    println!("═══════════════════════════════════════════════════════════════════");
    println!("Container Status");
    println!("═══════════════════════════════════════════════════════════════════");
    print_container_status(&runner)?;
    println!();
    println!("═══════════════════════════════════════════════════════════════════");
    println!("Health Checks");
    println!("═══════════════════════════════════════════════════════════════════");
    print_health_checks(&runner, context)?;
    println!();
    println!("═══════════════════════════════════════════════════════════════════");
    println!("Monitor Status");
    println!("═══════════════════════════════════════════════════════════════════");
    print_monitor_status(context, &env_config, &eth)?;
    println!();
    println!("═══════════════════════════════════════════════════════════════════");
    println!("Deployment Status");
    println!("═══════════════════════════════════════════════════════════════════");
    print_deployment_status(&env_config, deployments.as_ref(), &context.env_name);
    Ok(())
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
        let status = if runner
            .run(&CommandSpec::new("sh", vec!["-lc".to_string(), args]))
            .map(|output| output.success)
            .unwrap_or(false)
        {
            "OK"
        } else {
            "NOT RUNNING"
        };
        println!("{label:18} {status}");
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
            println!("Contracts: DEPLOYED ({})", env_config.active_provider);
            println!("  Source:");
            print_role_summary(deployments, crate::config::ChainRole::Source);
            println!("  Destination:");
            print_role_summary(deployments, crate::config::ChainRole::Destination);
        }
        _ => {
            println!(
                "Contracts: NOT DEPLOYED for '{}' (run 'make deploy ENV={}')",
                env_config.active_provider, env_name
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
        println!("oz-monitor lag: not tracked for non-local envs");
        return Ok(());
    }

    let runtime = runtime::RuntimeInputs::resolve(context, env_config);
    let Some(source_rpc) = runtime.source_rpc.as_deref().filter(|value| !value.is_empty()) else {
        println!("oz-monitor lag: unknown (missing source RPC)");
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
            println!("oz-monitor lag: {lag} block(s)");
        }
        (Some(_), None) => println!("oz-monitor lag: unknown (cursor missing)"),
        _ => println!("oz-monitor lag: unknown (cannot query source head)"),
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

fn health_commands(context: &ResolvedContext) -> Vec<(&'static str, String)> {
    let api_key = runtime::setting(context, "OZ_RELAYER_API_KEY");

    let mut commands = vec![
        (
            "operator-1:",
            "curl -sf http://localhost:3001/healthz >/dev/null".to_string(),
        ),
        (
            "operator-2:",
            "curl -sf http://localhost:3002/healthz >/dev/null".to_string(),
        ),
        (
            "operator-3:",
            "curl -sf http://localhost:3003/healthz >/dev/null".to_string(),
        ),
    ];

    let relayer_cmd = match api_key {
        Some(api_key) => format!(
            "curl -sf http://localhost:8080/api/v1/health -H 'Authorization: Bearer {api_key}' >/dev/null"
        ),
        None => "false".to_string(),
    };
    commands.push(("oz-relayer:", relayer_cmd));
    commands.extend([
        (
            "symbiotic-relay-1:",
            "curl -sf http://localhost:8081/healthz >/dev/null".to_string(),
        ),
        (
            "symbiotic-relay-2:",
            "curl -sf http://localhost:8082/healthz >/dev/null".to_string(),
        ),
        (
            "symbiotic-relay-3:",
            "curl -sf http://localhost:8083/healthz >/dev/null".to_string(),
        ),
    ]);
    commands
}
