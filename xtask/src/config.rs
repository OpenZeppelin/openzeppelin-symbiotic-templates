use std::fs;
use std::path::Path;

use eyre::{Result, eyre};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envfile;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConfig {
    pub version: u32,
    pub name: String,
    pub active_provider: Provider,
    pub chains: ChainsConfig,
    #[serde(default)]
    pub relay: RelayConfig,
    #[serde(default)]
    pub oz_monitor: Option<OzMonitorConfig>,
    #[serde(default)]
    pub oz_relayer: Option<OzRelayerConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Provider {
    #[serde(rename = "layerzero")]
    LayerZero,
    #[serde(rename = "chainlink_ccv")]
    ChainlinkCcv,
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
    pub rpc_urls: Vec<ConfigValue>,
    #[serde(default)]
    pub predeploys: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Plain(String),
    Tagged(TaggedConfigValue),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TaggedConfigValue {
    Plain { value: String },
    Env { value: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct RelayConfig {
    pub epoch_duration_seconds: u64,
    pub slashing_window_seconds: u64,
    pub epoch_start_delay_seconds: u64,
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
        let content = fs::read_to_string(path).map_err(|err| {
            eyre!(
                "failed to read environment config {}: {err}",
                path.display()
            )
        })?;
        serde_json::from_str(&content).map_err(|err| {
            eyre!(
                "failed to parse environment config {}: {err}",
                path.display()
            )
        })
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

impl ChainConfig {
    pub fn resolve_rpc_url(&self, project_root: &Path) -> Option<String> {
        self.rpc_urls
            .iter()
            .find_map(|value| value.resolve(project_root))
    }
}

impl ConfigValue {
    pub fn resolve(&self, project_root: &Path) -> Option<String> {
        match self {
            Self::Plain(value) => Some(value.clone()),
            Self::Tagged(TaggedConfigValue::Plain { value }) => Some(value.clone()),
            Self::Tagged(TaggedConfigValue::Env { value }) => envfile::get(project_root, value),
        }
        .filter(|value| !value.is_empty())
    }
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LayerZero => "layerzero",
            Self::ChainlinkCcv => "chainlink_ccv",
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl DeploymentsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|err| eyre!("failed to read deployments {}: {err}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|err| eyre!("failed to parse deployments {}: {err}", path.display()))
    }

    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        Self::load(path)
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

    fn empty() -> Self {
        Self {
            source: Value::Object(Default::default()),
            destination: Value::Object(Default::default()),
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
    use std::fs;

    use tempfile::tempdir;

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

    #[test]
    fn chain_rpc_url_prefers_first_resolved_entry() {
        let temp_dir = tempdir().unwrap();
        fs::write(
            temp_dir.path().join(".env"),
            "PRIMARY_RPC=\nSECONDARY_RPC=https://env.example\n",
        )
        .unwrap();

        let chain: ChainConfig = serde_json::from_str(
            r#"{
                "name": "src",
                "chainId": 1,
                "eid": 1,
                "confirmations": 1,
                "blockTimeMs": 1000,
                "rpcUrls": [
                    { "type": "env", "value": "PRIMARY_RPC" },
                    { "type": "env", "value": "SECONDARY_RPC" },
                    "https://plain.example"
                ],
                "predeploys": {}
            }"#,
        )
        .unwrap();

        assert_eq!(
            chain.resolve_rpc_url(temp_dir.path()).as_deref(),
            Some("https://env.example")
        );
    }

    #[test]
    fn chain_rpc_url_falls_back_to_plain_value() {
        let chain: ChainConfig = serde_json::from_str(
            r#"{
                "name": "src",
                "chainId": 1,
                "eid": 1,
                "confirmations": 1,
                "blockTimeMs": 1000,
                "rpcUrls": [
                    "https://plain.example"
                ],
                "predeploys": {}
            }"#,
        )
        .unwrap();

        assert_eq!(
            chain.resolve_rpc_url(Path::new(".")),
            Some("https://plain.example".to_string())
        );
    }
}
