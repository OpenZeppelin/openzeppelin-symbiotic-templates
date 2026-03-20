use std::fs;
use std::path::Path;

use eyre::{Result, eyre};
use serde::Deserialize;
use serde_json::Value;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConfig {
    pub version: u32,
    pub name: String,
    pub active_provider: String,
    pub chains: ChainsConfig,
    #[serde(default)]
    pub relay: RelayConfig,
    #[serde(default)]
    pub oz_monitor: Option<OzMonitorConfig>,
    #[serde(default)]
    pub oz_relayer: Option<OzRelayerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChainsConfig {
    pub source: ChainConfig,
    pub destination: ChainConfig,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub eid: u32,
    pub confirmations: u64,
    pub block_time_ms: u64,
    #[serde(default)]
    pub predeploys: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayConfig {
    pub epoch_duration_seconds: u64,
    pub slashing_window_seconds: u64,
    pub epoch_start_delay_seconds: u64,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            epoch_duration_seconds: 0,
            slashing_window_seconds: 0,
            epoch_start_delay_seconds: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OzMonitorConfig {
    pub cron_schedule: String,
    pub max_past_blocks: u64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OzRelayerConfig {
    pub default_speed: String,
    pub min_balance_wei: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentsConfig {
    pub source: Value,
    pub destination: Value,
}

#[derive(Debug, Clone, Copy)]
pub enum ChainRole {
    Source,
    Destination,
}

impl EnvironmentConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|err| eyre!("failed to read environment config {}: {err}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|err| eyre!("failed to parse environment config {}: {err}", path.display()))
    }

    pub fn is_local(&self) -> bool {
        self.chains.source.chain_id == 31_337
    }

    pub fn chain(&self, role: ChainRole) -> &ChainConfig {
        match role {
            ChainRole::Source => &self.chains.source,
            ChainRole::Destination => &self.chains.destination,
        }
    }

    pub fn predeploy(&self, role: ChainRole, namespace: &str, key: &str) -> Option<String> {
        self.chain(role)
            .predeploys
            .get(namespace)?
            .get(key)?
            .as_str()
            .map(ToOwned::to_owned)
    }
}

impl DeploymentsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|err| eyre!("failed to read deployments {}: {err}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|err| eyre!("failed to parse deployments {}: {err}", path.display()))
    }

    pub fn deployment(&self, role: ChainRole, key_path: &str) -> Option<String> {
        value_at(self.role(role), key_path)?
            .as_str()
            .map(ToOwned::to_owned)
    }

    pub fn role_has_entries(&self, role: ChainRole) -> bool {
        self.role(role)
            .as_object()
            .map(|items| !items.is_empty())
            .unwrap_or(false)
    }

    fn role(&self, role: ChainRole) -> &Value {
        match role {
            ChainRole::Source => &self.source,
            ChainRole::Destination => &self.destination,
        }
    }
}

fn value_at<'a>(value: &'a Value, key_path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in key_path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn deployment_lookup_supports_nested_keys() {
        let deployments: DeploymentsConfig = serde_json::from_str(
            r#"{
                "source": { "dvn": "0x1111111111111111111111111111111111111111" },
                "destination": { "relayInfra": { "settlement": "0x2222222222222222222222222222222222222222" } }
            }"#,
        )
        .unwrap();

        assert_eq!(
            deployments.deployment(ChainRole::Source, "dvn").as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            deployments
                .deployment(ChainRole::Destination, "relayInfra.settlement")
                .as_deref(),
            Some("0x2222222222222222222222222222222222222222")
        );
    }
}
