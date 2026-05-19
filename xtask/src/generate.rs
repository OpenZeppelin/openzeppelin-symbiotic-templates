use std::fs;
use std::path::Path;

#[cfg(test)]
use std::env;

use eyre::{Result, eyre};
use serde_json::{Value, json};

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::provider;
use crate::runtime::RuntimeInputs;
use crate::signer;

pub fn generate_runtime_artifacts(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    let deployments = DeploymentsConfig::load(&context.deployments)?;
    let runtime = RuntimeInputs::resolve(context, &env_config);

    let templates_root = context.project_root.join("config").join("templates");
    let static_relayer = context
        .project_root
        .join("config")
        .join("oz-relayer")
        .join("config.json");

    fs::create_dir_all(context.generated_dir.join("oz-monitor").join("networks"))?;
    fs::create_dir_all(context.generated_dir.join("oz-monitor").join("monitors"))?;
    fs::create_dir_all(context.generated_dir.join("oz-monitor").join("triggers"))?;
    fs::create_dir_all(context.generated_dir.join("oz-relayer").join("networks"))?;

    render_monitor_network(
        &env_config,
        &runtime,
        &templates_root,
        &context.generated_dir,
    )?;
    render_trigger_templates(context, &templates_root, &context.generated_dir)?;
    provider::render_monitor_definition(
        &env_config,
        &deployments,
        &templates_root,
        &context.generated_dir,
    )?;
    render_relayer(
        &env_config,
        &runtime,
        &static_relayer,
        &context.generated_dir,
    )?;
    render_sidecar_envs(
        &env_config,
        &deployments,
        &runtime,
        &context.generated_dir,
        context,
    )?;

    Ok(())
}

fn render_monitor_network(
    env_config: &EnvironmentConfig,
    runtime: &RuntimeInputs,
    templates_root: &Path,
    generated_dir: &Path,
) -> Result<()> {
    let output = generated_dir
        .join("oz-monitor")
        .join("networks")
        .join(if env_config.is_local() {
            "local_anvil.json"
        } else {
            "unused"
        });

    if env_config.is_local() {
        fs::copy(
            templates_root
                .join("oz-monitor")
                .join("networks")
                .join("local_anvil.json"),
            output,
        )?;
        return Ok(());
    }

    let monitor = env_config
        .oz_monitor
        .as_ref()
        .ok_or_else(|| eyre!("missing ozMonitor config in {}", env_config.name))?;
    let source_rpc = runtime
        .source_rpc
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("SOURCE RPC is required to render non-local monitor config"))?;
    let chain_id = env_config.chains.source.chain_id;
    let slug = format!("chain_{chain_id}");
    let network_json = json!({
        "slug": slug,
        "name": format!("Chain {chain_id}"),
        "network_type": "EVM",
        "chain_id": chain_id,
        "rpc_urls": [{
            "type_": "rpc",
            "url": { "type": "plain", "value": source_rpc },
            "weight": 100
        }],
        "block_time_ms": env_config.chains.source.block_time_ms,
        "confirmation_blocks": env_config.chains.source.confirmations,
        "cron_schedule": monitor.cron_schedule,
        "max_past_blocks": monitor.max_past_blocks,
        "store_blocks": false
    });

    write_pretty_json(
        &generated_dir
            .join("oz-monitor")
            .join("networks")
            .join(format!("{slug}.json")),
        &network_json,
    )
}

fn render_trigger_templates(
    context: &ResolvedContext,
    templates_root: &Path,
    generated_dir: &Path,
) -> Result<()> {
    let trigger_root = templates_root.join("oz-monitor").join("triggers");
    let output_root = generated_dir.join("oz-monitor").join("triggers");
    let webhook_secret = crate::runtime::setting(context, "WEBHOOK_SECRET")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("WEBHOOK_SECRET is required to render monitor webhook triggers"))?;

    for entry in fs::read_dir(&trigger_root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let mut trigger = read_json_value(&entry.path())?;
        inject_webhook_secret(&mut trigger, &webhook_secret);
        write_pretty_json(&output_root.join(entry.file_name()), &trigger)?;
    }

    Ok(())
}

fn inject_webhook_secret(value: &mut Value, webhook_secret: &str) {
    match value {
        Value::Object(object) => {
            if object
                .get("trigger_type")
                .and_then(Value::as_str)
                .is_some_and(|trigger_type| trigger_type == "webhook")
                && let Some(secret) = object
                    .get_mut("config")
                    .and_then(Value::as_object_mut)
                    .and_then(|config| config.get_mut("secret"))
                    .and_then(Value::as_object_mut)
            {
                secret.insert("type".to_string(), Value::String("plain".to_string()));
                secret.insert(
                    "value".to_string(),
                    Value::String(webhook_secret.to_string()),
                );
            }

            for child in object.values_mut() {
                inject_webhook_secret(child, webhook_secret);
            }
        }
        Value::Array(items) => {
            for item in items {
                inject_webhook_secret(item, webhook_secret);
            }
        }
        _ => {}
    }
}

fn render_relayer(
    env_config: &EnvironmentConfig,
    runtime: &RuntimeInputs,
    static_relayer: &Path,
    generated_dir: &Path,
) -> Result<()> {
    let output_config = generated_dir.join("oz-relayer").join("config.json");

    if env_config.is_local() {
        fs::copy(static_relayer, output_config)?;
        return Ok(());
    }

    let oz_relayer = env_config
        .oz_relayer
        .as_ref()
        .ok_or_else(|| eyre!("missing ozRelayer config in {}", env_config.name))?;
    let dest_rpc = runtime
        .dest_rpc
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("DEST RPC is required to render non-local relayer config"))?;
    let network_name = format!("chain-{}", env_config.chains.destination.chain_id);

    let network_json = json!({
        "networks": [{
            "type": "evm",
            "network": network_name,
            "chain_id": env_config.chains.destination.chain_id,
            "required_confirmations": env_config.chains.destination.confirmations,
            "symbol": "ETH",
            "rpc_urls": [dest_rpc],
            "explorer_urls": [],
            "average_blocktime_ms": env_config.chains.destination.block_time_ms,
            "is_testnet": env_config.chains.destination.is_testnet,
            "features": ["eip1559"]
        }]
    });
    write_pretty_json(
        &generated_dir
            .join("oz-relayer")
            .join("networks")
            .join("dest-network.json"),
        &network_json,
    )?;

    let mut relayer = read_json_value(static_relayer)?;
    if let Some(items) = relayer.get_mut("relayers").and_then(Value::as_array_mut) {
        let min_balance = oz_relayer
            .min_balance_wei
            .parse::<u64>()
            .map_err(|err| eyre!("invalid ozRelayer.minBalanceWei: {err}"))?;
        for item in items {
            item["network"] = Value::String(network_name.clone());
            item["policies"]["min_balance"] = Value::Number(min_balance.into());
        }
    }

    write_pretty_json(&output_config, &relayer)
}

fn render_sidecar_envs(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    generated_dir: &Path,
    context: &ResolvedContext,
) -> Result<()> {
    let driver = deployments
        .deployment(ChainRole::Destination, "relayInfra.driver")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source_chain_id = env_config.chains.source.chain_id;
    let dest_chain_id = env_config.chains.destination.chain_id;

    let mut rpc_lines = String::new();
    if !env_config.is_local() {
        let source_rpc = runtime
            .source_rpc
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("SOURCE RPC is required to render non-local sidecar config"))?;
        let dest_rpc = runtime
            .dest_rpc
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| eyre!("DEST RPC is required to render non-local sidecar config"))?;
        rpc_lines = format!("EVM_SOURCE_RPC={source_rpc}\nEVM_DEST_RPC={dest_rpc}\n");
    }

    let operators = env_config.operator_signers(&context.project_root, &context.env_name)?;

    for (i, operator) in operators.iter().enumerate() {
        let secret_keys = signer::build_sidecar_secret_keys(
            &operator.private_key,
            source_chain_id,
            dest_chain_id,
        );
        let output = generated_dir.join(format!("sidecar-{}.env", i + 1));
        let body = format!(
            "# Generated — do not edit\n\
             DRIVER_ADDRESS={driver}\n\
             DRIVER_CHAIN_ID={dest_chain_id}\n\
             SOURCE_CHAIN_ID={source_chain_id}\n\
             SIDECAR_SECRET_KEYS={secret_keys}\n\
             {rpc_lines}"
        );
        fs::write(output, body)?;
    }

    // Also write the legacy sidecar.env with shared vars (no secrets) for
    // any tooling that still reads it.
    let shared = format!(
        "# Generated — do not edit\n\
         DRIVER_ADDRESS={driver}\n\
         DRIVER_CHAIN_ID={dest_chain_id}\n\
         SOURCE_CHAIN_ID={source_chain_id}\n\
         {rpc_lines}"
    );
    fs::write(generated_dir.join("sidecar.env"), shared)?;

    Ok(())
}

pub(crate) fn read_json_value(path: &Path) -> Result<Value> {
    let body = fs::read_to_string(path)
        .map_err(|err| eyre!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&body).map_err(|err| eyre!("failed to parse {}: {err}", path.display()))
}

pub(crate) fn write_pretty_json(path: &Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    fs::write(path, format!("{body}\n"))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::context::ResolvedContext;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    fn write_context(env_body: &str, deployments_body: &str, env_name: &str) -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join(format!("{env_name}.json"));
        let deployments = root.join("deployments.json");
        let generated_dir = root.join("generated").join(env_name);
        fs::write(&env_config, env_body).unwrap();
        fs::write(&deployments, deployments_body).unwrap();
        std::mem::forget(temp_dir); // keep temp dir alive for test duration

        ResolvedContext {
            project_root: repo_root(),
            env_name: env_name.to_string(),
            env_config,
            deployments,
            generated_dir,
        }
    }

    #[test]
    fn render_local_layerzero_outputs_expected_files() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_context(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                },
                "signers": {
                    "deployer": { "type": "anvil", "index": 0 },
                    "operator-1": { "type": "anvil", "index": 1 },
                    "operator-2": { "type": "anvil", "index": 2 }
                },
                "ozMonitor": { "cronSchedule": "*/5 * * * * *", "maxPastBlocks": 50 },
                "ozRelayer": { "defaultSpeed": "fast", "minBalanceWei": "10000000000000000" },
                "funding": {
                    "operatorAmountWei": "1000000000000000000",
                    "signerAmountWei": "1000000000000000000",
                    "minBalanceThresholdWei": "1000000000000000000"
                }
            }"#,
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111" },
                "destination": { "relayInfra": { "driver": "0x2222222222222222222222222222222222222222" } }
            }"#,
            "local",
        );

        unsafe {
            env::set_var("WEBHOOK_SECRET", "local-webhook-secret-32-chars-min");
        }

        generate_runtime_artifacts(&context).unwrap();

        assert!(
            context
                .generated_dir
                .join("oz-monitor/networks/local_anvil.json")
                .exists()
        );
        assert!(
            context
                .generated_dir
                .join("oz-monitor/triggers/webhook_layerzero.json")
                .exists()
        );
        let triggers = fs::read_to_string(
            context
                .generated_dir
                .join("oz-monitor/triggers/webhook_layerzero.json"),
        )
        .unwrap();
        assert!(triggers.contains("local-webhook-secret-32-chars-min"));
        assert!(!triggers.contains("test-webhook-secret-32-chars-minimum"));
        assert!(
            context
                .generated_dir
                .join("oz-relayer/config.json")
                .exists()
        );

        let monitor = fs::read_to_string(
            context
                .generated_dir
                .join("oz-monitor/monitors/layerzero_job_assigned.json"),
        )
        .unwrap();
        assert!(monitor.contains("0x1111111111111111111111111111111111111111"));

        let sidecar = fs::read_to_string(context.generated_dir.join("sidecar.env")).unwrap();
        assert!(sidecar.contains("DRIVER_ADDRESS=0x2222222222222222222222222222222222222222"));
        assert!(sidecar.contains("DRIVER_CHAIN_ID=31338"));
        assert!(sidecar.contains("SOURCE_CHAIN_ID=31337"));

        let sidecar1 = fs::read_to_string(context.generated_dir.join("sidecar-1.env")).unwrap();
        assert!(sidecar1.contains("SIDECAR_SECRET_KEYS="));
        assert!(sidecar1.contains("DRIVER_CHAIN_ID=31338"));
        let sidecar2 = fs::read_to_string(context.generated_dir.join("sidecar-2.env")).unwrap();
        assert!(sidecar2.contains("SIDECAR_SECRET_KEYS="));
        assert!(!context.generated_dir.join("sidecar-3.env").exists());

        unsafe {
            env::remove_var("WEBHOOK_SECRET");
        }
    }

    #[test]
    fn render_non_local_chainlink_uses_overrides_and_generated_network_names() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_context(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "base-sepolia", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "sepolia", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                },
                "ozMonitor": { "cronSchedule": "*/15 * * * * *", "maxPastBlocks": 50 },
                "ozRelayer": { "defaultSpeed": "fast", "minBalanceWei": "10000000000000000" },
                "funding": {
                    "operatorAmountWei": "10000000000000000",
                    "signerAmountWei": "10000000000000000",
                    "minBalanceThresholdWei": "5000000000000000"
                }
            }"#,
            r#"{
                "source": { "chainlinkCcv": { "onRamp": "0x1111111111111111111111111111111111111111" } },
                "destination": { "relayInfra": { "driver": "0x2222222222222222222222222222222222222222" } }
            }"#,
            "testnet",
        );

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
            env::set_var("WEBHOOK_SECRET", "testnet-webhook-secret-32-chars");
            env::set_var(
                "CCV_SOURCE_ONRAMP_ADDRESS",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        }

        generate_runtime_artifacts(&context).unwrap();

        let network = fs::read_to_string(
            context
                .generated_dir
                .join("oz-monitor/networks/chain_84532.json"),
        )
        .unwrap();
        assert!(network.contains("\"chain_id\": 84532"));
        assert!(network.contains("https://source.example"));

        let monitor = fs::read_to_string(
            context
                .generated_dir
                .join("oz-monitor/monitors/ccip_message_sent.json"),
        )
        .unwrap();
        assert!(monitor.contains("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(monitor.contains("\"chain_84532\""));

        let relayer_network = fs::read_to_string(
            context
                .generated_dir
                .join("oz-relayer/networks/dest-network.json"),
        )
        .unwrap();
        assert!(relayer_network.contains("\"network\": \"chain-11155111\""));
        assert!(relayer_network.contains("https://dest.example"));

        let sidecar = fs::read_to_string(context.generated_dir.join("sidecar.env")).unwrap();
        assert!(sidecar.contains("EVM_SOURCE_RPC=https://source.example"));
        assert!(sidecar.contains("EVM_DEST_RPC=https://dest.example"));

        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
            env::remove_var("WEBHOOK_SECRET");
            env::remove_var("CCV_SOURCE_ONRAMP_ADDRESS");
        }
    }
}
