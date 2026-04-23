use std::env;
use std::path::Path;

use alloy::primitives::U256;
use eyre::{Result, bail};
use serde::Serialize;

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi, parse_address};
use crate::provider;
use crate::runtime::RuntimeInputs;
use crate::signer::SignerConfig;
use crate::signers;
use crate::ui;

pub fn run_command(context: &ResolvedContext, managed_operators: bool, json: bool) -> Result<()> {
    let eth = AlloyEth;
    let report = validate(context, managed_operators, &eth);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        ui::header("validate", &context.env_name, report.provider.as_deref());
        print_warnings(&report);
        if report.failures.is_empty() {
            ui::ok("validation passed");
        } else {
            print_failures(&report);
        }
    }

    if report.failures.is_empty() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

pub fn validate_or_bail<E: EthApi>(
    context: &ResolvedContext,
    managed_operators: bool,
    eth: &E,
) -> Result<()> {
    let report = validate(context, managed_operators, eth);
    if report.failures.is_empty() {
        print_warnings(&report);
        Ok(())
    } else {
        print_warnings(&report);
        print_failures(&report);
        bail!("runtime validation failed");
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub provider: Option<String>,
    pub warnings: Vec<String>,
    pub failures: Vec<String>,
}

pub fn validate<E: EthApi>(
    context: &ResolvedContext,
    managed_operators: bool,
    eth: &E,
) -> ValidationReport {
    let mut failures = Vec::new();
    let mut warnings = Vec::new();

    let env_config = load_required::<EnvironmentConfig>(
        &context.env_config,
        "environment config",
        &mut failures,
    );
    let deployments =
        load_required::<DeploymentsConfig>(&context.deployments, "deployments", &mut failures);

    let Some(env_config) = env_config else {
        return ValidationReport {
            provider: None,
            warnings,
            failures,
        };
    };
    let provider = Some(env_config.active_provider.to_string());
    let Some(deployments) = deployments else {
        return ValidationReport {
            provider,
            warnings,
            failures,
        };
    };

    let runtime = RuntimeInputs::resolve(context, &env_config);
    if !env_config.is_local() {
        runtime.validate_non_local_presence(&mut failures);
        validate_external_runtime(&env_config, &runtime, eth, &mut failures);
    }

    provider::validate_configuration(&env_config, &deployments, &mut failures, &mut warnings);
    provider::validate_chain_state(&env_config, &deployments, &runtime, eth, &mut failures);
    validate_genesis(&deployments, &runtime, eth, &mut failures);

    if managed_operators {
        validate_relayer_signers(context, &env_config, &runtime, eth, &mut failures);
        validate_managed_operator_keys(
            context,
            &env_config,
            &deployments,
            &runtime,
            eth,
            &mut failures,
        );
    }

    ValidationReport {
        provider,
        warnings,
        failures,
    }
}

fn print_failures(report: &ValidationReport) {
    ui::print_failures("validation failed", &report.failures);
}

fn print_warnings(report: &ValidationReport) {
    for warning in &report.warnings {
        ui::warn(warning);
    }
}

fn load_required<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
    failures: &mut Vec<String>,
) -> Option<T> {
    if !path.exists() {
        failures.push(format!("missing file: {}", path.display()));
        return None;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) => {
            failures.push(format!("failed to read {label} {}: {err}", path.display()));
            return None;
        }
    };

    match serde_json::from_str(&content) {
        Ok(value) => Some(value),
        Err(err) => {
            failures.push(format!("failed to parse {label} {}: {err}", path.display()));
            None
        }
    }
}

fn validate_external_runtime<E: EthApi>(
    env_config: &EnvironmentConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let Some(source_rpc) = runtime
        .source_rpc
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(dest_rpc) = runtime
        .dest_rpc
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(private_key) = runtime
        .private_key
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if let Some(SignerConfig::Anvil(_)) = env_config.signer("deployer") {
        failures.push(
            "deployer signer uses anvil type -- this will not work on external networks"
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

fn validate_genesis<E: EthApi>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let settlement = deployments.deployment(ChainRole::Destination, "relayInfra.settlement");
    let Some(dest_rpc) = runtime.dest_rpc.as_deref() else {
        return;
    };
    let Some(settlement) = settlement.as_deref() else {
        return;
    };
    let Some(settlement_address) = parse_address(settlement) else {
        return;
    };

    let Ok(epoch) = eth.last_committed_header_epoch(dest_rpc, settlement_address) else {
        failures.push("genesis missing: no committed settlement epoch found".to_string());
        return;
    };
    if epoch == 0 {
        failures.push("genesis missing: no committed settlement epoch found".to_string());
        return;
    }

    let Ok(capture) = eth.capture_timestamp(dest_rpc, settlement_address, epoch) else {
        failures.push(format!(
            "genesis invalid: settlement epoch {epoch} has no capture timestamp"
        ));
        return;
    };
    if capture == 0 {
        failures.push(format!(
            "genesis invalid: settlement epoch {epoch} has no capture timestamp"
        ));
        return;
    }

    let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => return,
    };
    let age = now.saturating_sub(capture);
    let max_age = env::var("MAX_EPOCH_VALIDITY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7200);

    if age >= max_age {
        failures.push(format!("genesis stale: age {age}s > {max_age}s"));
    }
}

fn validate_managed_operator_keys<E: EthApi>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let key_registry = deployments.deployment(ChainRole::Destination, "relayInfra.keyRegistry");
    let Some(dest_rpc) = runtime.dest_rpc.as_deref() else {
        return;
    };
    let Some(key_registry) = key_registry.as_deref() else {
        failures.push("missing relayInfra.keyRegistry in deployments file".to_string());
        return;
    };

    let operators = match env_config.operator_signers(&context.project_root, &context.env_name) {
        Ok(ops) => ops,
        Err(err) => {
            failures.push(format!("failed to resolve operator signers: {err}"));
            return;
        }
    };

    for (index, operator) in operators.iter().enumerate() {
        let operator_number = index + 1;
        let operator_address = operator.address;

        for tag in [15u8, 11u8] {
            let key = parse_address(key_registry)
                .and_then(|registry| {
                    eth.key_bytes(dest_rpc, registry, operator_address, tag)
                        .ok()
                })
                .unwrap_or_default();
            if key_is_missing(&key) {
                failures.push(format!(
                    "operator {operator_number} missing BLS key tag {tag}"
                ));
            }
        }

        let balance = eth.balance(dest_rpc, operator_address).unwrap_or_default();
        if balance == alloy::primitives::U256::ZERO {
            failures.push(format!(
                "operator {operator_number} has zero native balance on destination chain"
            ));
        }
    }
}

fn validate_relayer_signers<E: EthApi>(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    let passphrase = match signers::passphrase_from_context(context) {
        Ok(passphrase) => passphrase,
        Err(err) => {
            failures.push(err.to_string());
            return;
        }
    };

    let operator_addresses: Vec<(usize, alloy::primitives::Address)> = env_config
        .operator_signers(&context.project_root, &context.env_name)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, s)| (i + 1, s.address))
        .collect();

    let relayer_signers =
        match signers::load_signers_with_passphrase(&context.project_root, &passphrase) {
            Ok(signers) => signers,
            Err(err) => {
                failures.push(err.to_string());
                return;
            }
        };

    for signer in relayer_signers {
        let relayer_number = signer.number;
        let relayer_address = signer.address;

        if let Some(dest_rpc) = runtime.dest_rpc.as_deref() {
            let balance = eth.balance(dest_rpc, relayer_address).unwrap_or_default();
            if balance < U256::from(signers::MIN_RELAYER_NATIVE_BALANCE_WEI) {
                failures.push(format!(
                    "relayer signer {relayer_number} ({relayer_address}) has {} wei on destination chain; minimum required is {}",
                    balance,
                    signers::MIN_RELAYER_NATIVE_BALANCE_WEI
                ));
            }
        }

        let Some((operator_number, _)) = operator_addresses
            .iter()
            .find(|(_, operator_address)| *operator_address == relayer_address)
        else {
            continue;
        };

        failures.push(format!(
            "unsafe key overlap: relayer signer {relayer_number} address {relayer_address} matches operator {operator_number}"
        ));
    }
}

fn key_is_missing(key: &[u8]) -> bool {
    key.is_empty() || key.iter().all(|byte| *byte == 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::{env, fs};

    use tempfile::tempdir;

    use super::*;
    use crate::context::ResolvedContext;
    use crate::eth::AlloyEth;
    use crate::runner::FakeRunner;

    fn write_test_files(env_body: &str, deployments_body: &str, env_name: &str) -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join(format!("{env_name}.json"));
        let deployments = root.join("deployments.json");
        fs::write(&env_config, env_body).unwrap();
        fs::write(&deployments, deployments_body).unwrap();
        let leaked_root = temp_dir.path().to_path_buf();
        std::mem::forget(temp_dir); // keep temp dir alive for test duration

        ResolvedContext {
            project_root: leaked_root.clone(),
            env_name: env_name.to_string(),
            env_config,
            deployments,
            generated_dir: leaked_root.join("generated").join(env_name),
        }
    }

    fn local_env(provider: &str) -> String {
        format!(
            r#"{{
                "version": 1,
                "name": "local",
                "activeProvider": "{provider}",
                "chains": {{
                    "source": {{ "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {{}} }},
                    "destination": {{ "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {{}} }}
                }}
            }}"#
        )
    }

    fn non_local_layerzero_env_with_operator_signers() -> String {
        r#"{
            "version": 1,
            "name": "testnet",
            "activeProvider": "layerzero",
            "chains": {
                "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
            },
            "signers": {
                "deployer": { "type": "local", "path": "config/keys/deployer.json", "passphrase": { "type": "env", "value": "DEPLOYER_PASSPHRASE" } },
                "operator-1": { "type": "local", "path": "config/keys/operator-1.json", "passphrase": { "type": "env", "value": "OPERATOR_1_PASSPHRASE" } },
                "operator-2": { "type": "local", "path": "config/keys/operator-2.json", "passphrase": { "type": "env", "value": "OPERATOR_2_PASSPHRASE" } },
                "operator-3": { "type": "local", "path": "config/keys/operator-3.json", "passphrase": { "type": "env", "value": "OPERATOR_3_PASSPHRASE" } }
            }
        }"#.to_string()
    }

    fn setup_operator_keystores(root: &Path, keys: [&str; 3], passphrase: &str) {
        let keys_dir = root.join("config").join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        for (i, key) in keys.iter().enumerate() {
            crate::signers::write_keystore_from_private_key(
                &keys_dir,
                &format!("operator-{}", i + 1),
                passphrase,
                key,
            )
            .unwrap();
        }
    }

    fn setup_deployer_keystore(root: &Path, key: &str, passphrase: &str) {
        let keys_dir = root.join("config").join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        crate::signers::write_keystore_from_private_key(&keys_dir, "deployer", passphrase, key)
            .unwrap();
    }

    fn non_local_layerzero_deployments() -> &'static str {
        r#"{
            "source": {
                "dvn": "0x1111111111111111111111111111111111111111"
            },
            "destination": {
                "dvn": "0x3333333333333333333333333333333333333333",
                "relayInfra": {
                    "settlement": "0x5555555555555555555555555555555555555555",
                    "keyRegistry": "0x6666666666666666666666666666666666666666"
                }
            },
            "layerzero": {
                "oapp": {
                    "source": "0x2222222222222222222222222222222222222222",
                    "destination": "0x4444444444444444444444444444444444444444"
                }
            }
        }"#
    }

    fn bootstrap_relayer_signers(root: &Path, keys: [&str; 3], passphrase: &str) {
        let keys_dir = root.join("config").join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        for (i, key) in keys.iter().enumerate() {
            crate::signers::write_keystore_from_private_key(
                &keys_dir,
                &format!("signer-{}", i + 1),
                passphrase,
                key,
            )
            .unwrap();
        }
    }

    fn successful_non_local_layerzero_runner(
        operator_keys: [&str; 3],
        relayer_addresses: &[alloy::primitives::Address],
    ) -> FakeRunner {
        let operator_addresses: Vec<_> = operator_keys
            .iter()
            .map(|key| AlloyEth.address_from_private_key(key).unwrap())
            .collect();

        let mut runner = FakeRunner::default()
            .with_response(
                "cast",
                &["client", "--rpc-url", "https://source.example"],
                "",
            )
            .with_response("cast", &["client", "--rpc-url", "https://dest.example"], "")
            .with_response(
                "cast",
                &["chain-id", "--rpc-url", "https://source.example"],
                "84532",
            )
            .with_response(
                "cast",
                &["chain-id", "--rpc-url", "https://dest.example"],
                "11155111",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x1111111111111111111111111111111111111111",
                    "--rpc-url",
                    "https://source.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x2222222222222222222222222222222222222222",
                    "--rpc-url",
                    "https://source.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x3333333333333333333333333333333333333333",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x4444444444444444444444444444444444444444",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x5555555555555555555555555555555555555555",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x3333333333333333333333333333333333333333",
                    "settlement()(address)",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x5555555555555555555555555555555555555555",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x5555555555555555555555555555555555555555",
                    "getLastCommittedHeaderEpoch()(uint48)",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "1",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x5555555555555555555555555555555555555555",
                    "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)",
                    "1",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "18446744073709551615",
            );

        for (index, (key, address)) in operator_keys
            .iter()
            .zip(operator_addresses.iter())
            .enumerate()
        {
            runner = runner
                .with_response(
                    "cast",
                    &["wallet", "address", "--private-key", key],
                    &address.to_string(),
                )
                .with_response(
                    "cast",
                    &[
                        "call",
                        "0x6666666666666666666666666666666666666666",
                        "getKey(address,uint8)(bytes)",
                        &address.to_string(),
                        "15",
                        "--rpc-url",
                        "https://dest.example",
                    ],
                    "0x01",
                )
                .with_response(
                    "cast",
                    &[
                        "call",
                        "0x6666666666666666666666666666666666666666",
                        "getKey(address,uint8)(bytes)",
                        &address.to_string(),
                        "11",
                        "--rpc-url",
                        "https://dest.example",
                    ],
                    "0x01",
                )
                .with_response(
                    "cast",
                    &[
                        "balance",
                        &address.to_string(),
                        "--rpc-url",
                        "https://dest.example",
                    ],
                    "1",
                );
            let _ = index;
        }

        for address in relayer_addresses {
            runner = runner.with_response(
                "cast",
                &[
                    "balance",
                    &address.to_string(),
                    "--rpc-url",
                    "https://dest.example",
                ],
                "10000000000000000",
            );
        }

        runner
    }

    #[test]
    fn validate_reports_missing_layerzero_deployments() {
        let context = write_test_files(
            &local_env("layerzero"),
            r#"{ "source": {}, "destination": {} }"#,
            "local",
        );

        let report = validate(&context, false, &FakeRunner::default());

        assert!(
            report
                .failures
                .iter()
                .any(|item| item.contains("missing source DVN"))
        );
        assert!(
            report
                .failures
                .iter()
                .any(|item| item.contains("missing destination DVN"))
        );
    }

    #[test]
    fn validate_allows_chainlink_source_onramp_override() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_test_files(
            &local_env("chainlink_ccv"),
            r#"{
                "source": { "chainlinkCcv": { "ccv": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "offRamp": "0x3333333333333333333333333333333333333333" } },
                "destination": { "chainlinkCcv": { "ccv": "0xcccccccccccccccccccccccccccccccccccccccc", "onRamp": "0x4444444444444444444444444444444444444444", "offRamp": "0x2222222222222222222222222222222222222222" } }
            }"#,
            "local",
        );

        let no_override = validate(&context, false, &FakeRunner::default());
        assert!(
            no_override
                .failures
                .iter()
                .any(|item| item.contains("missing CCV source onRamp"))
        );

        unsafe {
            env::set_var(
                "CCV_SOURCE_ONRAMP_ADDRESS",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        }

        let with_override = validate(&context, false, &FakeRunner::default());
        assert!(
            !with_override
                .failures
                .iter()
                .any(|item| item.contains("missing CCV source onRamp"))
        );

        unsafe {
            env::remove_var("CCV_SOURCE_ONRAMP_ADDRESS");
        }
    }

    #[test]
    fn validate_reports_chainlink_settlement_mismatch() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_test_files(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 84532, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 11155111, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                }
            }"#,
            r#"{
                "source": {
                    "chainlinkCcv": {
                        "ccv": "0x1111111111111111111111111111111111111111",
                        "onRamp": "0x2222222222222222222222222222222222222222"
                    }
                },
                "destination": {
                    "chainlinkCcv": {
                        "ccv": "0x3333333333333333333333333333333333333333",
                        "offRamp": "0x4444444444444444444444444444444444444444",
                        "settlement": "0x5555555555555555555555555555555555555555"
                    }
                }
            }"#,
            "testnet",
        );

        let runner = FakeRunner::default()
            .with_response(
                "cast",
                &[
                    "code",
                    "0x1111111111111111111111111111111111111111",
                    "--rpc-url",
                    "https://source.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x3333333333333333333333333333333333333333",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x2222222222222222222222222222222222222222",
                    "--rpc-url",
                    "https://source.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x4444444444444444444444444444444444444444",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x5555555555555555555555555555555555555555",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x3333333333333333333333333333333333333333",
                    "settlement()(address)",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "0x6666666666666666666666666666666666666666",
            );

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
        }

        let report = validate(&context, false, &runner);
        assert!(
            report
                .failures
                .iter()
                .any(|item| item.contains("settlement mismatch"))
        );

        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
        }
    }

    #[test]
    fn non_local_validation_rejects_anvil_signer_type() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let context = write_test_files(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                },
                "signers": {
                    "deployer": { "type": "anvil", "index": 0 }
                }
            }"#,
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111" },
                "destination": { "dvn": "0x3333333333333333333333333333333333333333" },
                "layerzero": {
                    "oapp": {
                        "source": "0x2222222222222222222222222222222222222222",
                        "destination": "0x4444444444444444444444444444444444444444"
                    }
                }
            }"#,
            "testnet",
        );

        let anvil_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let runner = FakeRunner::default()
            .with_response(
                "cast",
                &["client", "--rpc-url", "https://source.example"],
                "",
            )
            .with_response("cast", &["client", "--rpc-url", "https://dest.example"], "")
            .with_response(
                "cast",
                &["chain-id", "--rpc-url", "https://source.example"],
                "84532",
            )
            .with_response(
                "cast",
                &["chain-id", "--rpc-url", "https://dest.example"],
                "11155111",
            )
            .with_response(
                "cast",
                &["wallet", "address", "--private-key", anvil_key],
                "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
            )
            .with_response(
                "cast",
                &[
                    "balance",
                    "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
                    "--rpc-url",
                    "https://source.example",
                ],
                "1",
            )
            .with_response(
                "cast",
                &[
                    "balance",
                    "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
                    "--rpc-url",
                    "https://dest.example",
                ],
                "1",
            );

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
        }

        let report = validate(&context, false, &runner);
        assert!(
            report
                .failures
                .iter()
                .any(|item| item.contains("anvil type"))
        );

        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
        }
    }

    #[test]
    fn validate_reports_zeroed_bls_key_as_missing() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let operator_keys = [
            "0x0000000000000000000000000000000000000000000000000000000000001001",
            "0x0000000000000000000000000000000000000000000000000000000000001002",
            "0x0000000000000000000000000000000000000000000000000000000000001003",
        ];
        let passphrase = "test-passphrase";
        let relayer_addresses = [
            "0x9000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            "0x9000000000000000000000000000000000000002"
                .parse()
                .unwrap(),
            "0x9000000000000000000000000000000000000003"
                .parse()
                .unwrap(),
        ];
        let zero_key = "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        let operator_1_address = AlloyEth.address_from_private_key(operator_keys[0]).unwrap();
        let context = write_test_files(
            &non_local_layerzero_env_with_operator_signers(),
            non_local_layerzero_deployments(),
            "testnet",
        );
        setup_operator_keystores(&context.project_root, operator_keys, passphrase);
        setup_deployer_keystore(
            &context.project_root,
            "0x0000000000000000000000000000000000000000000000000000000000001234",
            passphrase,
        );
        bootstrap_relayer_signers(
            &context.project_root,
            [
                "0x0000000000000000000000000000000000000000000000000000000000002001",
                "0x0000000000000000000000000000000000000000000000000000000000002002",
                "0x0000000000000000000000000000000000000000000000000000000000002003",
            ],
            "keystore-passphrase",
        );
        unsafe {
            env::set_var("KEYSTORE_PASSPHRASE", "keystore-passphrase");
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
            env::set_var("DEPLOYER_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_1_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_2_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_3_PASSPHRASE", passphrase);
        }

        let deployer_address = AlloyEth
            .address_from_private_key(
                "0x0000000000000000000000000000000000000000000000000000000000001234",
            )
            .unwrap();
        let runner = successful_non_local_layerzero_runner(operator_keys, &relayer_addresses)
            .with_response(
                "cast",
                &[
                    "wallet",
                    "address",
                    "--private-key",
                    "0x0000000000000000000000000000000000000000000000000000000000001234",
                ],
                &deployer_address.to_string(),
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x6666666666666666666666666666666666666666",
                    "getKey(address,uint8)(bytes)",
                    &operator_1_address.to_string(),
                    "15",
                    "--rpc-url",
                    "https://dest.example",
                ],
                zero_key,
            );

        let report = validate(&context, true, &runner);

        assert!(
            report
                .failures
                .iter()
                .any(|item| item == "operator 1 missing BLS key tag 15"),
            "expected 'operator 1 missing BLS key tag 15' in failures: {:?}",
            report.failures
        );

        unsafe {
            env::remove_var("KEYSTORE_PASSPHRASE");
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
            env::remove_var("DEPLOYER_PASSPHRASE");
            env::remove_var("OPERATOR_1_PASSPHRASE");
            env::remove_var("OPERATOR_2_PASSPHRASE");
            env::remove_var("OPERATOR_3_PASSPHRASE");
        }
    }

    #[test]
    fn validate_reports_explicit_relayer_operator_overlap() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let passphrase = "test-passphrase";
        let context = write_test_files(
            &non_local_layerzero_env_with_operator_signers(),
            non_local_layerzero_deployments(),
            "testnet",
        );
        let operator_keys = [
            "0x1111111111111111111111111111111111111111111111111111111111111111",
            "0x2222222222222222222222222222222222222222222222222222222222222222",
            "0x3333333333333333333333333333333333333333333333333333333333333333",
        ];
        let relayer_keys = [
            "0x2222222222222222222222222222222222222222222222222222222222222222",
            "0x4444444444444444444444444444444444444444444444444444444444444444",
            "0x5555555555555555555555555555555555555555555555555555555555555555",
        ];

        setup_operator_keystores(&context.project_root, operator_keys, passphrase);
        bootstrap_relayer_signers(context.project_root.as_path(), relayer_keys, passphrase);
        let relayer_addresses = crate::signers::load_signers_with_passphrase(
            context.project_root.as_path(),
            passphrase,
        )
        .unwrap()
        .into_iter()
        .map(|signer| signer.address)
        .collect::<Vec<_>>();
        let overlap_address = AlloyEth.address_from_private_key(operator_keys[1]).unwrap();
        let runner = successful_non_local_layerzero_runner(operator_keys, &relayer_addresses);

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
            env::set_var("KEYSTORE_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_1_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_2_PASSPHRASE", passphrase);
            env::set_var("OPERATOR_3_PASSPHRASE", passphrase);
        }

        let report = validate(&context, true, &runner);
        assert!(
            report.failures.iter().any(|item| {
                item.contains(&format!("relayer signer 1 address {overlap_address}"))
                    && item.contains("matches operator 2")
            }),
            "expected operator overlap failure in: {:?}",
            report.failures
        );

        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
            env::remove_var("KEYSTORE_PASSPHRASE");
            env::remove_var("OPERATOR_1_PASSPHRASE");
            env::remove_var("OPERATOR_2_PASSPHRASE");
            env::remove_var("OPERATOR_3_PASSPHRASE");
        }
    }

    #[test]
    fn validate_warns_when_layerzero_oapp_is_disabled() {
        let _guard = crate::runtime::test_env_lock()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        unsafe {
            env::remove_var("SOURCE_RPC_URL");
            env::remove_var("DEST_RPC_URL");
        }
        let context = write_test_files(
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                },
                "layerzero": {
                    "oapp": {
                        "enabled": false
                    }
                }
            }"#,
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111" },
                "destination": {
                    "dvn": "0x3333333333333333333333333333333333333333",
                    "relayInfra": {
                        "settlement": "0x5555555555555555555555555555555555555555"
                    }
                }
            }"#,
            "local",
        );

        let runner = FakeRunner::default()
            .with_response(
                "cast",
                &[
                    "code",
                    "0x1111111111111111111111111111111111111111",
                    "--rpc-url",
                    "http://localhost:8545",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x3333333333333333333333333333333333333333",
                    "--rpc-url",
                    "http://localhost:8546",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x5555555555555555555555555555555555555555",
                    "--rpc-url",
                    "http://localhost:8546",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x3333333333333333333333333333333333333333",
                    "settlement()(address)",
                    "--rpc-url",
                    "http://localhost:8546",
                ],
                "0x5555555555555555555555555555555555555555",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x5555555555555555555555555555555555555555",
                    "getLastCommittedHeaderEpoch()(uint48)",
                    "--rpc-url",
                    "http://localhost:8546",
                ],
                "1",
            )
            .with_response(
                "cast",
                &[
                    "call",
                    "0x5555555555555555555555555555555555555555",
                    "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)",
                    "1",
                    "--rpc-url",
                    "http://localhost:8546",
                ],
                "18446744073709551615",
            );

        let report = validate(&context, false, &runner);
        assert!(report.failures.is_empty());
        assert!(
            report
                .warnings
                .iter()
                .any(|item| item.contains("starter OApp is disabled"))
        );
    }
}
