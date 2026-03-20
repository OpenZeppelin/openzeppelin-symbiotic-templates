use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask")]
#[command(about = "Internal control-plane automation for the repo")]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Clone, Args, Default)]
pub struct GlobalArgs {
    /// Environment name. Defaults to ENV or `local`.
    #[arg(long, global = true)]
    pub env: Option<String>,

    /// Override environment config path.
    #[arg(long, global = true)]
    pub env_config: Option<PathBuf>,

    /// Override deployments file path.
    #[arg(long, global = true)]
    pub deployments: Option<PathBuf>,

    /// Override generated output directory.
    #[arg(long, global = true)]
    pub generated_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Deploy or reconcile contracts for the selected environment.
    Deploy,
    /// Run read-only validation checks.
    Validate(ValidateArgs),
    /// Start the full local stack.
    StartLocal,
    /// Start non-local operator services.
    RunOperators,
    /// Clear generated/local runtime state.
    Clean,
    /// Show local service and deployment status.
    Status,
    /// Pass through to the current message helper.
    Msg(MsgArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ValidateArgs {
    /// Include managed-operator checks.
    #[arg(long)]
    pub managed_operators: bool,

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MsgArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::unwrap_err_used)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn parse_validate_with_global_overrides() {
        let cli = Cli::try_parse_from([
            "xtask",
            "--env",
            "testnet",
            "--env-config",
            "config/environments/testnet.json",
            "--deployments",
            "deployments/testnet.json",
            "validate",
            "--managed-operators",
        ])
        .unwrap();

        assert_eq!(cli.global.env.as_deref(), Some("testnet"));
        assert_eq!(
            cli.global.env_config.as_deref(),
            Some(PathBuf::from("config/environments/testnet.json").as_path())
        );
        assert!(matches!(
            cli.command,
            Commands::Validate(ValidateArgs {
                managed_operators: true,
                json: false
            })
        ));
    }

    #[test]
    fn parse_start_local() {
        let cli = Cli::try_parse_from(["xtask", "start-local"]).unwrap();
        assert!(matches!(cli.command, Commands::StartLocal));
    }

    #[test]
    fn msg_requires_subcommand_args_positionally() {
        let cli = Cli::try_parse_from(["xtask", "msg", "send", "--message", "hello"]).unwrap();
        match cli.command {
            Commands::Msg(args) => {
                assert_eq!(args.args, vec!["send", "--message", "hello"]);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn command_is_required() {
        let err = Cli::try_parse_from(["xtask"]).unwrap_err();
        assert!(matches!(
            err.kind(),
            ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ));
    }
}
