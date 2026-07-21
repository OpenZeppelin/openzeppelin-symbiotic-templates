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
mod generate;
mod runner;
mod runtime;
mod services;
mod signer;
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
        Commands::Chains(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            start::run_chains(&context, args.fresh)?;
        }
        Commands::Validate(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            validate::run_command(&context, args.managed_operators, args.json)?;
        }
        Commands::Deploy => {
            let context = ResolvedContext::from_global(&cli.global)?;
            deploy::run_command(&context)?;
        }
        Commands::Finalize => {
            let context = ResolvedContext::from_global(&cli.global)?;
            deploy::finalize(&context)?;
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
        Commands::GenerateSigner(args) => {
            let context = ResolvedContext::from_global(&cli.global)?;
            let passphrase = signer::resolve_passphrase(args.passphrase.as_deref())?;
            let keys_dir = context
                .project_root
                .join("config")
                .join("keys")
                .join(&context.env_name);
            for name in &args.name {
                let resolved = signer::generate_keystore(&keys_dir, name, &passphrase)?;
                println!("  {name}: {} ({})", resolved.address, keys_dir.join(format!("{name}.json")).display());
            }
        }
    }

    Ok(())
}
