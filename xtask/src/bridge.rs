use std::process::Command;

use eyre::{Result, eyre};

use crate::context::ResolvedContext;

pub fn run_make_target(context: &ResolvedContext, target: &str) -> Result<()> {
    let mut command = Command::new("make");
    command
        .current_dir(&context.project_root)
        .arg(target)
        .args(context.make_overrides());

    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("`make {target}` failed with status {status}"))
    }
}

pub fn run_msg(context: &ResolvedContext, args: &[String]) -> Result<()> {
    let status = Command::new(context.project_root.join("scripts/msg"))
        .current_dir(&context.project_root)
        .env("ENV", &context.env_name)
        .env("ENV_CONFIG", &context.env_config)
        .env("DEPLOYMENTS_FILE", &context.deployments)
        .env("GENERATED_DIR", &context.generated_dir)
        .args(args)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre!("`scripts/msg` failed with status {status}"))
    }
}
