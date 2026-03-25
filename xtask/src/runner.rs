use std::process::Command;

use thiserror::Error;

use crate::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            envs: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("failed to spawn `{program}`: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

pub trait CommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunnerError>;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunnerError> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.envs.iter().map(|(key, value)| (key, value)));
        let output = ui::run_command(&mut command, &format!("still running {}", spec.program))
            .map_err(|source| RunnerError::Io {
                program: spec.program.clone(),
                source,
            })?;

        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct FakeRunner {
    responses: std::collections::HashMap<String, CommandOutput>,
}

#[cfg(test)]
impl FakeRunner {
    pub fn with_response(mut self, program: &str, args: &[&str], stdout: &str) -> Self {
        self.responses.insert(
            key(program, args),
            CommandOutput {
                success: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            },
        );
        self
    }
}

#[cfg(test)]
impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, RunnerError> {
        Ok(self
            .responses
            .get(&key(
                &spec.program,
                &spec.args.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .cloned()
            .unwrap_or(CommandOutput {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            }))
    }
}

#[cfg(test)]
fn key(program: &str, args: &[&str]) -> String {
    format!("{program}\u{1f}{sep}", sep = args.join("\u{1f}"))
}
