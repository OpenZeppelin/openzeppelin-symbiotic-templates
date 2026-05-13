use std::fs;

use eyre::Result;

use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::runner::SystemRunner;
use crate::services;
use crate::ui;

pub fn run_command(context: &ResolvedContext) -> Result<()> {
    let env_config = EnvironmentConfig::load(&context.env_config)?;
    clean_inner(context, &env_config, true)
}

fn clean_inner(
    context: &ResolvedContext,
    env_config: &EnvironmentConfig,
    run_docker: bool,
) -> Result<()> {
    if run_docker {
        ui::header(
            "clean",
            &context.env_name,
            Some(env_config.active_provider.as_str()),
        );
    }

    if run_docker {
        let runner = SystemRunner;

        let stop = ui::step("clear local runtime state");
        let _ = services::down(&runner, context, env_config, true);
        stop.done("local runtime state cleared");
    }

    remove_dir_all_if_exists(context.project_root.join("data"))?;
    remove_dir_all_if_exists(context.project_root.join("generated"))?;

    if env_config.is_local() {
        remove_file_if_exists(&context.deployments)?;
        remove_dir_all_if_exists(context.project_root.join("contracts").join("broadcast"))?;
        if run_docker {
            ui::ok("clean complete");
            ui::next("make deploy");
        }
    } else if run_docker {
        ui::ok("clean complete");
        ui::next(&format!("make deploy ENV={}", context.env_name));
    }

    Ok(())
}

fn remove_dir_all_if_exists(path: impl AsRef<std::path::Path>) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn remove_file_if_exists(path: impl AsRef<std::path::Path>) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn local_clean_removes_generated_data_and_local_deployments() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let env_config = root.join("local.json");
        let deployments = root.join("deployments").join("local.json");
        let generated = root.join("generated").join("local");
        let data = root.join("data").join("foo");

        fs::create_dir_all(deployments.parent().unwrap()).unwrap();
        fs::create_dir_all(&generated).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(
            &env_config,
            r#"{
                "version": 1,
                "name": "local",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "anvil", "chainId": 31337, "eid": 31337, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} },
                    "destination": { "name": "anvil-settlement", "chainId": 31338, "eid": 31338, "confirmations": 1, "blockTimeMs": 1000, "predeploys": {} }
                },
                "funding": {
                    "operatorAmountWei": "1000000000000000000",
                    "signerAmountWei": "1000000000000000000",
                    "minBalanceThresholdWei": "1000000000000000000"
                }
            }"#,
        )
        .unwrap();
        fs::write(&deployments, "{}").unwrap();

        let context = ResolvedContext {
            project_root: root.clone(),
            env_name: "local".to_string(),
            env_config,
            deployments: deployments.clone(),
            generated_dir: root.join("generated").join("local"),
        };

        let env_config = EnvironmentConfig::load(&context.env_config).unwrap();
        clean_inner(&context, &env_config, false).unwrap();

        assert!(!root.join("data").exists());
        assert!(!root.join("generated").exists());
        assert!(!deployments.exists());
    }
}
