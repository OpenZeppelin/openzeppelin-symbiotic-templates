use std::env;

use eyre::Result;

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi};
use crate::runtime::{DEFAULT_ANVIL_PRIVATE_KEY, RuntimeInputs};

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let eth = AlloyEth;
    let report = preflight(context, &eth);

    if report.failures.is_empty() {
        println!(
            "Preflight checks passed for provider: {}",
            report.provider.as_deref().unwrap_or("unknown")
        );
        Ok(())
    } else {
        eprintln!("Preflight checks failed:");
        for failure in &report.failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}

#[derive(Debug, Clone)]
pub struct PreflightReport {
    pub provider: Option<String>,
    pub failures: Vec<String>,
}

pub fn preflight<E: EthApi>(context: &ResolvedContext, eth: &E) -> PreflightReport {
    let mut failures = Vec::new();

    let env_config = match EnvironmentConfig::load(&context.env_config) {
        Ok(config) => config,
        Err(err) => {
            failures.push(err.to_string());
            return PreflightReport {
                provider: None,
                failures,
            };
        }
    };
    let deployments = match DeploymentsConfig::load(&context.deployments) {
        Ok(config) => config,
        Err(err) => {
            failures.push(err.to_string());
            return PreflightReport {
                provider: Some(env_config.active_provider.clone()),
                failures,
            };
        }
    };

    let runtime = RuntimeInputs::resolve(context, &env_config);
    if !env_config.is_local() {
        runtime.validate_non_local_presence(&mut failures);
        validate_external_network(&env_config, &runtime, eth, &mut failures);
    }

    require_role_entries(&deployments, ChainRole::Source, &mut failures);
    require_role_entries(&deployments, ChainRole::Destination, &mut failures);

    match env_config.active_provider.as_str() {
        "layerzero" => validate_layerzero_preflight(&deployments, &mut failures),
        "chainlink_ccv" => validate_chainlink_preflight(&env_config, &deployments, &mut failures),
        other => failures.push(format!("unsupported provider for preflight: {other}")),
    }

    PreflightReport {
        provider: Some(env_config.active_provider.clone()),
        failures,
    }
}

fn validate_external_network<E: EthApi>(
    env_config: &EnvironmentConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let Some(source_rpc) = runtime.source_rpc.as_deref().filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(dest_rpc) = runtime.dest_rpc.as_deref().filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(private_key) = runtime.private_key.as_deref().filter(|value| !value.is_empty()) else {
        return;
    };

    if private_key == DEFAULT_ANVIL_PRIVATE_KEY {
        failures.push(
            "PRIVATE_KEY is set to the default Anvil key -- this will not work on external networks"
                .to_string(),
        );
    }

    if !eth.rpc_reachable(source_rpc) {
        failures.push(format!("cannot reach source RPC: {source_rpc}"));
    }
    if !eth.rpc_reachable(dest_rpc) {
        failures.push(format!("cannot reach destination RPC: {dest_rpc}"));
    }

    if let Ok(actual) = eth.chain_id(source_rpc)
        && actual != env_config.chains.source.chain_id
    {
        failures.push(format!(
            "source chain ID mismatch: RPC reports {actual}, config expects {}",
            env_config.chains.source.chain_id
        ));
    }
    if let Ok(actual) = eth.chain_id(dest_rpc)
        && actual != env_config.chains.destination.chain_id
    {
        failures.push(format!(
            "destination chain ID mismatch: RPC reports {actual}, config expects {}",
            env_config.chains.destination.chain_id
        ));
    }

    let Ok(deployer_address) = eth.address_from_private_key(private_key) else {
        failures.push("invalid PRIVATE_KEY".to_string());
        return;
    };

    for (label, rpc) in [("source", source_rpc), ("destination", dest_rpc)] {
        match eth.balance(rpc, deployer_address) {
            Ok(balance) if balance > alloy::primitives::U256::ZERO => {}
            _ => failures.push(format!(
                "deployer {deployer_address} has zero balance on {label} chain ({rpc})"
            )),
        }
    }
}

fn require_role_entries(
    deployments: &DeploymentsConfig,
    role: ChainRole,
    failures: &mut Vec<String>,
) {
    if deployments.role_has_entries(role) {
        return;
    }

    let label = match role {
        ChainRole::Source => "source",
        ChainRole::Destination => "destination",
    };
    failures.push(format!("no {label} deployments in deployments file. Run `make deploy`."));
}

fn validate_layerzero_preflight(
    deployments: &DeploymentsConfig,
    failures: &mut Vec<String>,
) {
    require_deployment(
        deployments.deployment(ChainRole::Source, "dvn"),
        "missing source DVN deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "dvn"),
        "missing destination DVN deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Source, "testOApp"),
        "missing source TestOApp deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "testOApp"),
        "missing destination TestOApp deployment in deployments file",
        failures,
    );
}

fn validate_chainlink_preflight(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    failures: &mut Vec<String>,
) {
    require_deployment(
        deployments.deployment(ChainRole::Source, "chainlinkCcv.ccv"),
        "missing source CCV deployment in deployments file",
        failures,
    );
    require_deployment(
        deployments.deployment(ChainRole::Destination, "chainlinkCcv.ccv"),
        "missing destination CCV deployment in deployments file",
        failures,
    );

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
            resolve_env_or_deployment("CCV_SOURCE_ONRAMP_ADDRESS", deployments, ChainRole::Source, "chainlinkCcv.onRamp"),
            "missing CCV source onRamp. Set CCV_SOURCE_ONRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV source onRamp address",
        ),
        (
            resolve_env_or_deployment("CCV_SOURCE_OFFRAMP_ADDRESS", deployments, ChainRole::Source, "chainlinkCcv.offRamp"),
            "missing CCV source offRamp. Set CCV_SOURCE_OFFRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV source offRamp address",
        ),
        (
            resolve_env_or_deployment("CCV_DEST_ONRAMP_ADDRESS", deployments, ChainRole::Destination, "chainlinkCcv.onRamp"),
            "missing CCV destination onRamp. Set CCV_DEST_ONRAMP_ADDRESS or deploy CCV contracts.",
            "invalid CCV destination onRamp address",
        ),
        (
            resolve_env_or_deployment("CCV_DEST_OFFRAMP_ADDRESS", deployments, ChainRole::Destination, "chainlinkCcv.offRamp"),
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

}

fn validate_chain_selector(
    env_var: &str,
    default: u64,
    label: &str,
    failures: &mut Vec<String>,
) {
    match env::var(env_var) {
        Ok(value) if value.is_empty() => failures.push(format!("invalid {label}: ''")),
        Ok(value) if value.parse::<u64>().is_err() => {
            failures.push(format!("invalid {label}: '{value}'"));
        }
        Ok(_) | Err(env::VarError::NotPresent) => {
            let _ = default;
        }
        Err(env::VarError::NotUnicode(_)) => failures.push(format!("invalid {label}: non-utf8 value")),
    }
}

fn resolve_env_or_deployment(
    env_var: &str,
    deployments: &DeploymentsConfig,
    role: ChainRole,
    key_path: &str,
) -> Option<String> {
    env::var(env_var)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| deployments.deployment(role, key_path))
}

fn require_deployment(value: Option<String>, message: &str, failures: &mut Vec<String>) {
    let Some(value) = value else {
        failures.push(message.to_string());
        return;
    };
    if value.is_empty() || value == "null" {
        failures.push(message.to_string());
    }
}

fn is_hex_address(value: &str) -> bool {
    value.len() == 42 && value.starts_with("0x") && value[2..].chars().all(|ch| ch.is_ascii_hexdigit())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::context::ResolvedContext;
    use crate::runner::FakeRunner;

    fn write_context(
        env_body: &str,
        deployments_body: &str,
        env_name: &str,
    ) -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join(format!("{env_name}.json"));
        let deployments = root.join("deployments.json");
        let generated_dir = root.join("generated").join(env_name);
        fs::write(&env_config, env_body).unwrap();
        fs::write(&deployments, deployments_body).unwrap();
        std::mem::forget(temp_dir);

        ResolvedContext {
            project_root: root.clone(),
            env_name: env_name.to_string(),
            env_config,
            deployments,
            generated_dir,
        }
    }

    #[test]
    fn local_chainlink_preflight_accepts_complete_config() {
        let context = write_context(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                }
            }"#,
            r#"{
                "source": { "chainlinkCcv": { "ccv": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "onRamp": "0x1111111111111111111111111111111111111111", "offRamp": "0x3333333333333333333333333333333333333333" } },
                "destination": { "chainlinkCcv": { "ccv": "0xcccccccccccccccccccccccccccccccccccccccc", "onRamp": "0x4444444444444444444444444444444444444444", "offRamp": "0x2222222222222222222222222222222222222222" } }
            }"#,
            "local",
        );

        let report = preflight(&context, &FakeRunner::default());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn chainlink_preflight_allows_source_onramp_override() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_context(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                }
            }"#,
            r#"{
                "source": { "chainlinkCcv": { "ccv": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "offRamp": "0x3333333333333333333333333333333333333333" } },
                "destination": { "chainlinkCcv": { "ccv": "0xcccccccccccccccccccccccccccccccccccccccc", "onRamp": "0x4444444444444444444444444444444444444444", "offRamp": "0x2222222222222222222222222222222222222222" } }
            }"#,
            "local",
        );

        let no_override = preflight(&context, &FakeRunner::default());
        assert!(no_override
            .failures
            .iter()
            .any(|item| item.contains("missing CCV source onRamp")));

        unsafe {
            env::set_var(
                "CCV_SOURCE_ONRAMP_ADDRESS",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        }

        let with_override = preflight(&context, &FakeRunner::default());
        assert!(!with_override
            .failures
            .iter()
            .any(|item| item.contains("missing CCV source onRamp")));

        unsafe {
            env::remove_var("CCV_SOURCE_ONRAMP_ADDRESS");
        }
    }

    #[test]
    fn non_local_preflight_rejects_default_anvil_key() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_context(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                }
            }"#,
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111", "testOApp": "0x2222222222222222222222222222222222222222" },
                "destination": { "dvn": "0x3333333333333333333333333333333333333333", "testOApp": "0x4444444444444444444444444444444444444444" }
            }"#,
            "testnet",
        );

        let runner = FakeRunner::default()
            .with_response("cast", &["client", "--rpc-url", "https://source.example"], "")
            .with_response("cast", &["client", "--rpc-url", "https://dest.example"], "")
            .with_response("cast", &["chain-id", "--rpc-url", "https://source.example"], "84532")
            .with_response("cast", &["chain-id", "--rpc-url", "https://dest.example"], "11155111")
            .with_response(
                "cast",
                &[
                    "wallet",
                    "address",
                    "--private-key",
                    DEFAULT_ANVIL_PRIVATE_KEY,
                ],
                "0x9999999999999999999999999999999999999999",
            )
            .with_response(
                "cast",
                &[
                    "balance",
                    "0x9999999999999999999999999999999999999999",
                    "--rpc-url",
                    "https://source.example",
                ],
                "1",
            )
            .with_response(
                "cast",
                &[
                    "balance",
                    "0x9999999999999999999999999999999999999999",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "1",
            );

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
            env::set_var("PRIVATE_KEY", DEFAULT_ANVIL_PRIVATE_KEY);
        }

        let report = preflight(&context, &runner);
        assert!(report
            .failures
            .iter()
            .any(|item| item.contains("default Anvil key")));

        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
            env::remove_var("PRIVATE_KEY");
        }
    }
}
