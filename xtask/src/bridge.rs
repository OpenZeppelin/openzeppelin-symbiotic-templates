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
