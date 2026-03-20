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
}

#[allow(dead_code)]
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
    pub fn is_local(&self) -> bool {
        self.chains.source.chain_id == 31_337
    }
}

impl DeploymentsConfig {
    pub fn deployment(&self, role: ChainRole, key_path: &str) -> Option<String> {
        value_at(self.role(role), key_path)?
            .as_str()
            .map(ToOwned::to_owned)
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
