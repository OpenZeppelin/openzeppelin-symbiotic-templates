use std::env;
use std::path::Path;

use eyre::Result;
use serde::Serialize;

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};
use crate::context::ResolvedContext;
use crate::runner::{CommandRunner, CommandSpec, SystemRunner};

pub fn run_command(context: &ResolvedContext, managed_operators: bool, json: bool) -> Result<()> {
    let runner = SystemRunner;
    let report = validate(context, managed_operators, &runner);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.failures.is_empty() {
        println!(
            "Validation passed for provider: {}",
            report.provider.as_deref().unwrap_or("unknown")
        );
    } else {
        eprintln!("Validation failed:");
        for failure in &report.failures {
            eprintln!("  - {failure}");
        }
    }

    if report.failures.is_empty() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub provider: Option<String>,
    pub failures: Vec<String>,
}

pub fn validate<R: CommandRunner>(
    context: &ResolvedContext,
    managed_operators: bool,
    runner: &R,
) -> ValidationReport {
    let mut failures = Vec::new();

    let env_config = load_required::<EnvironmentConfig>(&context.env_config, "environment config", &mut failures);
    let deployments = load_required::<DeploymentsConfig>(&context.deployments, "deployments", &mut failures);

    let Some(env_config) = env_config else {
        return ValidationReport {
            provider: None,
            failures,
        };
    };
    let provider = Some(env_config.active_provider.clone());
    let Some(deployments) = deployments else {
        return ValidationReport { provider, failures };
    };

    let runtime = RuntimeInputs::resolve(&env_config, &mut failures);

    match env_config.active_provider.as_str() {
        "layerzero" => validate_layerzero(&deployments, &runtime, runner, &mut failures),
        "chainlink_ccv" => validate_chainlink_ccv(&deployments, &runtime, runner, &mut failures),
        other => failures.push(format!("unsupported provider: {other}")),
    }

    validate_genesis(&deployments, &runtime, runner, &mut failures);

    if managed_operators {
        validate_managed_operator_keys(&deployments, &runtime, runner, &mut failures);
    }

    ValidationReport { provider, failures }
}

#[derive(Debug, Clone)]
struct RuntimeInputs {
    source_rpc: Option<String>,
    dest_rpc: Option<String>,
}

impl RuntimeInputs {
    fn resolve(env_config: &EnvironmentConfig, failures: &mut Vec<String>) -> Self {
        if env_config.is_local() {
            return Self {
                source_rpc: Some("http://localhost:8545".to_string()),
                dest_rpc: Some("http://localhost:8546".to_string()),
            };
        }

        let source_rpc = env::var("SOURCE_RPC_URL").ok();
        let dest_rpc = env::var("DEST_RPC_URL").ok();
        let private_key = env::var("PRIVATE_KEY").ok();

        if source_rpc.as_deref().unwrap_or_default().is_empty() {
            failures.push("SOURCE RPC is not configured".to_string());
        }
        if dest_rpc.as_deref().unwrap_or_default().is_empty() {
            failures.push("DEST RPC is not configured".to_string());
        }
        if private_key.as_deref().unwrap_or_default().is_empty() {
            failures.push("PRIVATE_KEY is not configured".to_string());
        }

        Self {
            source_rpc,
            dest_rpc,
        }
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

fn validate_layerzero<R: CommandRunner>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    runner: &R,
    failures: &mut Vec<String>,
) {
    let src_dvn = deployments.deployment(ChainRole::Source, "dvn");
    let dst_dvn = deployments.deployment(ChainRole::Destination, "dvn");
    let src_oapp = deployments.deployment(ChainRole::Source, "testOApp");
    let dst_oapp = deployments.deployment(ChainRole::Destination, "testOApp");
    let settlement = deployments.deployment(ChainRole::Destination, "relayInfra.settlement");

    check_code(
        runtime.source_rpc.as_deref(),
        src_dvn.as_deref(),
        "source DVN",
        runner,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_dvn.as_deref(),
        "destination DVN",
        runner,
        failures,
    );
    check_code(
        runtime.source_rpc.as_deref(),
        src_oapp.as_deref(),
        "source TestOApp",
        runner,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_oapp.as_deref(),
        "destination TestOApp",
        runner,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        settlement.as_deref(),
        "relayInfra.settlement",
        runner,
        failures,
    );

    if let (Some(dest_rpc), Some(dst_dvn), Some(settlement)) =
        (runtime.dest_rpc.as_deref(), dst_dvn.as_deref(), settlement.as_deref())
        && is_hex_address(dst_dvn)
        && is_hex_address(settlement)
    {
        let actual = cast_output(
            runner,
            "cast",
            vec![
                "call".to_string(),
                dst_dvn.to_string(),
                "settlement()(address)".to_string(),
                "--rpc-url".to_string(),
                dest_rpc.to_string(),
            ],
        );
        if let Some(actual) = actual
            && !actual.is_empty()
            && lower_hex(&actual) != lower_hex(settlement)
        {
            failures.push(format!(
                "destination DVN settlement mismatch: expected {settlement}, got {}",
                lower_hex(&actual)
            ));
        }
    }
}

fn validate_chainlink_ccv<R: CommandRunner>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    runner: &R,
    failures: &mut Vec<String>,
) {
    let src_ccv = deployments.deployment(ChainRole::Source, "chainlinkCcv.ccv");
    let dst_ccv = deployments.deployment(ChainRole::Destination, "chainlinkCcv.ccv");
    let src_onramp = deployments.deployment(ChainRole::Source, "chainlinkCcv.onRamp");
    let dst_offramp = deployments.deployment(ChainRole::Destination, "chainlinkCcv.offRamp");
    let settlement = deployments.deployment(ChainRole::Destination, "chainlinkCcv.settlement");

    check_code(
        runtime.source_rpc.as_deref(),
        src_ccv.as_deref(),
        "source CCV",
        runner,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_ccv.as_deref(),
        "destination CCV",
        runner,
        failures,
    );
    check_code(
        runtime.source_rpc.as_deref(),
        src_onramp.as_deref(),
        "source onRamp",
        runner,
        failures,
    );
    check_code(
        runtime.dest_rpc.as_deref(),
        dst_offramp.as_deref(),
        "destination offRamp",
        runner,
        failures,
    );

    if let Some(settlement) = settlement.as_deref()
        && !settlement.is_empty()
    {
        check_code(
            runtime.dest_rpc.as_deref(),
            Some(settlement),
            "destination CCV settlement",
            runner,
            failures,
        );
        if let (Some(dest_rpc), Some(dst_ccv)) = (runtime.dest_rpc.as_deref(), dst_ccv.as_deref()) {
            let actual = cast_output(
                runner,
                "cast",
                vec![
                    "call".to_string(),
                    dst_ccv.to_string(),
                    "settlement()(address)".to_string(),
                    "--rpc-url".to_string(),
                    dest_rpc.to_string(),
                ],
            );
            if let Some(actual) = actual
                && !actual.is_empty()
                && lower_hex(&actual) != lower_hex(settlement)
            {
                failures.push(format!(
                    "destination CCV settlement mismatch: expected {settlement}, got {}",
                    lower_hex(&actual)
                ));
            }
        }
    }
}

fn validate_genesis<R: CommandRunner>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    runner: &R,
    failures: &mut Vec<String>,
) {
    let settlement = deployments.deployment(ChainRole::Destination, "relayInfra.settlement");
    let Some(dest_rpc) = runtime.dest_rpc.as_deref() else {
        return;
    };
    let Some(settlement) = settlement.as_deref() else {
        return;
    };
    if !is_hex_address(settlement) {
        return;
    }

    let epoch = cast_output(
        runner,
        "cast",
        vec![
            "call".to_string(),
            settlement.to_string(),
            "getLastCommittedHeaderEpoch()(uint48)".to_string(),
            "--rpc-url".to_string(),
            dest_rpc.to_string(),
        ],
    )
    .unwrap_or_else(|| "0".to_string());

    let Ok(epoch) = epoch.parse::<u64>() else {
        failures.push("genesis missing: no committed settlement epoch found".to_string());
        return;
    };
    if epoch == 0 {
        failures.push("genesis missing: no committed settlement epoch found".to_string());
        return;
    }

    let capture = cast_output(
        runner,
        "cast",
        vec![
            "call".to_string(),
            settlement.to_string(),
            "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)".to_string(),
            epoch.to_string(),
            "--rpc-url".to_string(),
            dest_rpc.to_string(),
        ],
    )
    .unwrap_or_else(|| "0".to_string());

    let Ok(capture) = capture.parse::<u64>() else {
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

fn validate_managed_operator_keys<R: CommandRunner>(
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    runner: &R,
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

    for index in 0..3 {
        let operator_number = index + 1;
        let env_var = format!("OPERATOR_{operator_number}_PRIVATE_KEY");
        let Some(private_key) = env::var(&env_var).ok().filter(|value| !value.is_empty()) else {
            failures.push(format!("managed operator {operator_number} key missing"));
            continue;
        };

        let Some(operator_address) = cast_output(
            runner,
            "cast",
            vec![
                "wallet".to_string(),
                "address".to_string(),
                "--private-key".to_string(),
                private_key,
            ],
        ) else {
            failures.push(format!("managed operator {operator_number} key missing"));
            continue;
        };

        for tag in [15u8, 11u8] {
            let key = cast_output(
                runner,
                "cast",
                vec![
                    "call".to_string(),
                    key_registry.to_string(),
                    "getKey(address,uint8)(bytes)".to_string(),
                    operator_address.clone(),
                    tag.to_string(),
                    "--rpc-url".to_string(),
                    dest_rpc.to_string(),
                ],
            )
            .unwrap_or_default();
            if key.is_empty() || key == "0x" {
                failures.push(format!("operator {operator_number} missing BLS key tag {tag}"));
            }
        }

        let balance = cast_output(
            runner,
            "cast",
            vec![
                "balance".to_string(),
                operator_address,
                "--rpc-url".to_string(),
                dest_rpc.to_string(),
            ],
        )
        .unwrap_or_else(|| "0".to_string());
        if balance == "0" {
            failures.push(format!(
                "operator {operator_number} has zero native balance on destination chain"
            ));
        }
    }
}

fn check_code<R: CommandRunner>(
    rpc_url: Option<&str>,
    address: Option<&str>,
    label: &str,
    runner: &R,
    failures: &mut Vec<String>,
) {
    let Some(address) = address else {
        failures.push(format!("missing {label} in deployments file"));
        return;
    };
    if !is_hex_address(address) {
        failures.push(format!("invalid {label}: {address}"));
        return;
    }
    let Some(rpc_url) = rpc_url else {
        return;
    };

    let code = cast_output(
        runner,
        "cast",
        vec![
            "code".to_string(),
            address.to_string(),
            "--rpc-url".to_string(),
            rpc_url.to_string(),
        ],
    )
    .unwrap_or_else(|| "0x".to_string());
    if code.is_empty() || code == "0x" {
        failures.push(format!("{label} has no code at {address}"));
    }
}

fn cast_output<R: CommandRunner>(runner: &R, program: &str, args: Vec<String>) -> Option<String> {
    let output = runner.run(&CommandSpec::new(program, args)).ok()?;
    if !output.success {
        return None;
    }
    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn lower_hex(value: &str) -> String {
    value.to_ascii_lowercase()
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

    fn write_test_files(env_body: &str, deployments_body: &str) -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join("local.json");
        let deployments = root.join("deployments.json");
        fs::write(&env_config, env_body).unwrap();
        fs::write(&deployments, deployments_body).unwrap();
        let leaked_root = temp_dir.path().to_path_buf();
        std::mem::forget(temp_dir);

        ResolvedContext {
            project_root: leaked_root.clone(),
            env_name: "local".to_string(),
            env_config,
            deployments,
            generated_dir: leaked_root.join("generated").join("local"),
        }
    }

    fn local_env(provider: &str) -> String {
        format!(
            r#"{{
                "version": 1,
                "name": "local",
                "activeProvider": "{provider}",
                "chains": {{
                    "source": {{ "name": "anvil", "chainId": 31337, "eid": 31337 }},
                    "destination": {{ "name": "anvil-settlement", "chainId": 31338, "eid": 31338 }}
                }}
            }}"#
        )
    }

    #[test]
    fn validate_reports_missing_layerzero_deployments() {
        let context = write_test_files(
            &local_env("layerzero"),
            r#"{ "source": {}, "destination": {} }"#,
        );

        let report = validate(&context, false, &FakeRunner::default());

        assert!(report
            .failures
            .iter()
            .any(|item| item == "missing source DVN in deployments file"));
        assert!(report
            .failures
            .iter()
            .any(|item| item == "missing destination DVN in deployments file"));
    }

    #[test]
    fn validate_reports_chainlink_settlement_mismatch() {
        let context = write_test_files(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "chainlink_ccv",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 84532 },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 11155111 }
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
                "0x9999999999999999999999999999999999999999",
            );

        unsafe {
            env::set_var("SOURCE_RPC_URL", "https://source.example");
            env::set_var("DEST_RPC_URL", "https://dest.example");
            env::set_var(
                "PRIVATE_KEY",
                "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            );
        }

        let report = validate(&context, false, &runner);

        assert!(report.failures.iter().any(|item| {
            item.contains("destination CCV settlement mismatch")
                && item.contains("0x9999999999999999999999999999999999999999")
        }));
    }

    #[test]
    fn validate_reports_missing_managed_operator_keys() {
        let context = write_test_files(
            &local_env("layerzero"),
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111", "testOApp": "0x2222222222222222222222222222222222222222" },
                "destination": {
                    "dvn": "0x3333333333333333333333333333333333333333",
                    "testOApp": "0x4444444444444444444444444444444444444444",
                    "relayInfra": {
                        "settlement": "0x5555555555555555555555555555555555555555",
                        "keyRegistry": "0x6666666666666666666666666666666666666666"
                    }
                }
            }"#,
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
                    "0x2222222222222222222222222222222222222222",
                    "--rpc-url",
                    "http://localhost:8545",
                ],
                "0x1234",
            )
            .with_response(
                "cast",
                &[
                    "code",
                    "0x4444444444444444444444444444444444444444",
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
                "9999999999",
            );

        unsafe {
            env::remove_var("OPERATOR_1_PRIVATE_KEY");
            env::remove_var("OPERATOR_2_PRIVATE_KEY");
            env::remove_var("OPERATOR_3_PRIVATE_KEY");
        }

        let report = validate(&context, true, &runner);

        assert!(
            report
                .failures
                .iter()
                .any(|item| item == "managed operator 1 key missing")
        );
        assert!(
            report
                .failures
                .iter()
                .any(|item| item == "managed operator 2 key missing")
        );
        assert!(
            report
                .failures
                .iter()
                .any(|item| item == "managed operator 3 key missing")
        );
    }
}
