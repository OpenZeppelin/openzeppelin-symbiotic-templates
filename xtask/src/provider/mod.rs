pub mod chainlink_ccv;
pub mod layerzero;

use std::path::Path;

use eyre::Result;

use crate::config::{DeploymentsConfig, EnvironmentConfig, Provider};
use crate::context::ResolvedContext;
use crate::eth::EthApi;
use crate::runtime::RuntimeInputs;

pub fn deploy(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    match env_config.active_provider {
        Provider::LayerZero => layerzero::deploy(context, env_config),
        Provider::ChainlinkCcv => chainlink_ccv::deploy(context, env_config),
    }
}

pub fn validate_configuration(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    failures: &mut Vec<String>,
) {
    match env_config.active_provider {
        Provider::LayerZero => layerzero::validate_configuration(env_config, deployments, failures),
        Provider::ChainlinkCcv => {
            chainlink_ccv::validate_configuration(env_config, deployments, failures)
        }
    }
}

pub fn validate_chain_state<E: EthApi>(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    runtime: &RuntimeInputs,
    eth: &E,
    failures: &mut Vec<String>,
) {
    match env_config.active_provider {
        Provider::LayerZero => layerzero::validate_chain_state(deployments, runtime, eth, failures),
        Provider::ChainlinkCcv => {
            chainlink_ccv::validate_chain_state(deployments, runtime, eth, failures)
        }
    }
}

pub fn render_monitor_definition(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    templates_root: &Path,
    generated_dir: &Path,
) -> Result<()> {
    match env_config.active_provider {
        Provider::LayerZero => layerzero::render_monitor_definition(
            env_config,
            deployments,
            templates_root,
            generated_dir,
        ),
        Provider::ChainlinkCcv => chainlink_ccv::render_monitor_definition(
            env_config,
            deployments,
            templates_root,
            generated_dir,
        ),
    }
}

pub fn configure_startup(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Result<()> {
    match env_config.active_provider {
        Provider::LayerZero => Ok(()),
        Provider::ChainlinkCcv => chainlink_ccv::configure_startup(context, env_config),
    }
}
