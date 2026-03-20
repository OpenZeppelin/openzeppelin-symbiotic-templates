use clap::Parser;
use eyre::Result;

mod bridge;
mod cli;
mod config;
mod context;
mod runner;
mod validate;

use cli::{Cli, Commands};
use context::ResolvedContext;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Validate(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            validate::run_command(&context, args.managed_operators, args.json)?;
        }
        Commands::Deploy => {
            let context = ResolvedContext::from_global(&cli.global)?;
            bridge::run_make_target(&context, "deploy")?;
        }
        Commands::StartLocal => {
            let context = ResolvedContext::for_forced_env(&cli.global, "local")?;
            bridge::run_make_target(&context, "start")?;
        }
        Commands::RunOperators => {
            let context = ResolvedContext::from_global(&cli.global)?;
            if context.env_name == "local" {
                eyre::bail!("run-operators is non-local only; use `cargo xtask start-local`");
            }
            bridge::run_make_target(&context, "run-operators")?;
        }
        Commands::Clean => {
            let context = ResolvedContext::from_global(&cli.global)?;
            bridge::run_make_target(&context, "clean")?;
        }
        Commands::Status => {
            let context = ResolvedContext::from_global(&cli.global)?;
            bridge::run_make_target(&context, "status")?;
        }
        Commands::Msg(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            bridge::run_msg(&context, &args.args)?;
        }
    }

    Ok(())
}
