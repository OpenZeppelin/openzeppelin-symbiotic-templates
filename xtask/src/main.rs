use clap::Parser;
use eyre::Result;

mod bridge;
mod clean;
mod cli;
mod config;
mod context;
mod deploy;
mod eth;
mod envfile;
mod genesis;
mod msg;
mod operators;
mod preflight;
mod publish;
mod render;
mod runner;
mod runtime;
mod services;
mod start;
mod status;
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
        Commands::PublishAddresses => {
            let context = ResolvedContext::from_global(&cli.global)?;
            publish::run_command(&context)?;
        }
        Commands::Preflight => {
            let context = ResolvedContext::from_global(&cli.global)?;
            preflight::run_command(&context)?;
        }
        Commands::Render => {
            let context = ResolvedContext::from_global(&cli.global)?;
            render::run_command(&context)?;
        }
        Commands::Deploy => {
            let context = ResolvedContext::from_global(&cli.global)?;
            deploy::run_command(&context)?;
        }
        Commands::StartLocal => {
            let context = ResolvedContext::for_forced_env(&cli.global, "local")?;
            start::run_start_local(&context)?;
        }
        Commands::RunOperators => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_run_operators(&context)?;
        }
        Commands::Clean => {
            let context = ResolvedContext::from_global(&cli.global)?;
            clean::run_command(&context)?;
        }
        Commands::Status => {
            let context = ResolvedContext::from_global(&cli.global)?;
            status::run_command(&context)?;
        }
        Commands::Msg(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            msg::run_command(&context, &args)?;
        }
    }

    Ok(())
}
