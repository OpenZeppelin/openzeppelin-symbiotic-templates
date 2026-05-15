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
    /// Start local chains (Anvil). No-op for non-local environments.
    Chains,
    /// Deploy the selected stack for the environment.
    Deploy,
    /// Run post-deploy file generation against existing on-chain state.
    /// Recovers from a `deploy` that bailed after contracts landed.
    Finalize,
    /// Refresh committed settlement genesis without redeploying contracts.
    RefreshGenesis,
    /// Run read-only validation checks.
    Validate(ValidateArgs),
    /// Start the stack (local or non-local, detected from environment config).
    Start(StartArgs),
    /// Start the full local stack (deprecated: use `start`).
    #[command(hide = true)]
    StartLocal,
    /// Start non-local operator services (deprecated: use `start`).
    #[command(hide = true)]
    RunOperators,
    /// Clear generated/local runtime state.
    Clean,
    /// Show local service and deployment status.
    Status,
    /// Send and verify test messages.
    Msg(MsgArgs),
    /// Generate new encrypted keystore(s) in config/keys/<env>/.
    GenerateSigner(GenerateSignerArgs),
}

#[derive(Debug, Clone, Args)]
pub struct GenerateSignerArgs {
    /// Signer name(s). Each creates a `<name>.json` keystore.
    #[arg(long, required = true)]
    pub name: Vec<String>,

    /// Keystore encryption passphrase. If omitted, prompts interactively.
    #[arg(long)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct StartArgs {
    /// Reset local runtime state before starting (wipes sidecar data, monitor
    /// cursors, and restarts infra from scratch). Local environments only.
    #[arg(long)]
    pub reset: bool,
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
    #[command(subcommand)]
    pub command: MsgCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum MsgCommand {
    /// Send one test message.
    Send(MsgSendArgs),
    /// Watch a previously sent message until it lands on destination.
    Watch(MsgWatchArgs),
    /// Send a message, then watch it to completion.
    E2e(MsgE2eArgs),
}

#[derive(Debug, Clone, Args)]
pub struct MsgSendArgs {
    /// Message payload.
    #[arg(default_value = "hello")]
    pub message: String,

    /// Destination executor gas limit.
    #[arg(long, default_value_t = 200_000)]
    pub gas: u128,

    /// Emit JSON instead of human output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MsgWatchArgs {
    /// Message ID to watch. Falls back to the last sent message cache.
    #[arg(long)]
    pub id: Option<String>,

    /// Source tx hash to watch. Falls back to the last sent message cache.
    #[arg(long)]
    pub tx: Option<String>,

    /// Timeout in seconds.
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,

    /// Emit JSON instead of human output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MsgE2eArgs {
    /// Message payload.
    #[arg(default_value = "hello")]
    pub message: String,

    /// Destination executor gas limit.
    #[arg(long, default_value_t = 200_000)]
    pub gas: u128,

    /// Timeout in seconds.
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,

    /// Emit JSON instead of human output.
    #[arg(long)]
    pub json: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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
    fn parse_start() {
        let cli = Cli::try_parse_from(["xtask", "start"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Start(StartArgs { reset: false })
        ));
    }

    #[test]
    fn parse_start_with_reset() {
        let cli = Cli::try_parse_from(["xtask", "start", "--reset"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Start(StartArgs { reset: true })
        ));
    }

    #[test]
    fn parse_start_local_deprecated() {
        let cli = Cli::try_parse_from(["xtask", "start-local"]).unwrap();
        assert!(matches!(cli.command, Commands::StartLocal));
    }

    #[test]
    fn parse_refresh_genesis() {
        let cli = Cli::try_parse_from(["xtask", "refresh-genesis"]).unwrap();
        assert!(matches!(cli.command, Commands::RefreshGenesis));
    }

    #[test]
    fn parse_finalize() {
        let cli = Cli::try_parse_from(["xtask", "finalize"]).unwrap();
        assert!(matches!(cli.command, Commands::Finalize));
    }

    #[test]
    fn msg_parses_structured_subcommands() {
        let cli =
            Cli::try_parse_from(["xtask", "msg", "send", "hello", "--gas", "250000"]).unwrap();
        match cli.command {
            Commands::Msg(args) => match args.command {
                MsgCommand::Send(send) => {
                    assert_eq!(send.message, "hello");
                    assert_eq!(send.gas, 250_000);
                    assert!(!send.json);
                }
                other => panic!("unexpected msg subcommand: {other:?}"),
            },
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

    #[test]
    fn parse_generate_signer() {
        let cli = Cli::try_parse_from([
            "xtask",
            "generate-signer",
            "--name",
            "deployer",
            "--name",
            "operator-1",
            "--passphrase",
            "test-pass",
        ])
        .unwrap();

        match cli.command {
            Commands::GenerateSigner(args) => {
                assert_eq!(args.name, vec!["deployer", "operator-1"]);
                assert_eq!(args.passphrase.as_deref(), Some("test-pass"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn generate_signer_requires_name() {
        let err =
            Cli::try_parse_from(["xtask", "generate-signer", "--passphrase", "test"]).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::MissingRequiredArgument));
    }
}
