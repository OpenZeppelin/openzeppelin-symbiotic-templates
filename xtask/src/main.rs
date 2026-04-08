use clap::Parser;
use eyre::Result;

mod addresses;
mod clean;
mod cli;
mod config;
mod context;
mod deploy;
mod envfile;
mod eth;
mod genesis;
mod msg;
mod provider;
mod publish;
mod render;
mod runner;
mod runtime;
mod services;
mod signers;
mod start;
mod status;
mod ui;
mod validate;

use cli::{Cli, Commands};
use context::ResolvedContext;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Chains => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_chains(&context)?;
        }
        Commands::Validate(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            validate::run_command(&context, args.managed_operators, args.json)?;
        }
        Commands::Deploy => {
            let context = ResolvedContext::from_global(&cli.global)?;
            deploy::run_command(&context)?;
        }
        Commands::RefreshGenesis => {
            let context = ResolvedContext::from_global(&cli.global)?;
            genesis::run_command(&context)?;
        }
        Commands::Start(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_start(&context, args.reset)?;
        }
        // Deprecated aliases — forward to unified start
        Commands::StartLocal => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_start(&context, true)?;
        }
        Commands::RunOperators => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_start(&context, false)?;
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
        Commands::BootstrapRelayerSigners => {
            let context = ResolvedContext::from_global(&cli.global)?;
            signers::run_bootstrap_command(&context.project_root, &context.env_name)?;
        }
    }

    Ok(())
}
