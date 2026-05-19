use eyre::{Result, eyre};

use crate::config::{ChainRole, DeploymentsConfig, EnvironmentConfig};

pub fn resolve(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    role: ChainRole,
    deployment_key: &str,
    predeploy: Option<(&str, &str)>,
    label: &str,
) -> Result<Option<String>> {
    let deployment = deployments
        .deployment(role, deployment_key)
        .filter(|value| !value.is_empty());
    let configured = predeploy
        .and_then(|(namespace, key)| env_config.predeploy(role, namespace, key))
        .filter(|value| !value.is_empty());

    if let (Some(deployment), Some(configured)) = (&deployment, &configured)
        && !deployment.eq_ignore_ascii_case(configured)
    {
        return Err(eyre!(
            "{label} drift: deployments has {deployment}, predeploys has {configured}"
        ));
    }

    Ok(deployment.or(configured))
}

pub fn require(
    env_config: &EnvironmentConfig,
    deployments: &DeploymentsConfig,
    role: ChainRole,
    deployment_key: &str,
    predeploy: Option<(&str, &str)>,
    label: &str,
) -> Result<String> {
    resolve(
        env_config,
        deployments,
        role,
        deployment_key,
        predeploy,
        label,
    )?
    .ok_or_else(|| eyre!("missing {label}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::config::ChainRole;
    use crate::config::{DeploymentsConfig, EnvironmentConfig};

    use super::*;

    fn env_config() -> EnvironmentConfig {
        serde_json::from_str(
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": {
                        "name": "dst",
                        "chainId": 11155111,
                        "eid": 40161,
                        "confirmations": 3,
                        "blockTimeMs": 12000,
                        "predeploys": {
                            "symbioticCore": {
                                "settlement": "0x2222222222222222222222222222222222222222"
                            }
                        }
                    }
                },
                "funding": {
                    "operatorAmountWei": "5000000000000000",
                    "signerAmountWei": "5000000000000000",
                    "minBalanceThresholdWei": "5000000000000000"
                }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn resolve_prefers_deployments() {
        let deployments: DeploymentsConfig = serde_json::from_str(
            r#"{
                "source": {},
                "destination": {
                    "relayInfra": {
                        "settlement": "0x2222222222222222222222222222222222222222"
                    }
                }
            }"#,
        )
        .unwrap();

        let resolved = resolve(
            &env_config(),
            &deployments,
            ChainRole::Destination,
            "relayInfra.settlement",
            Some(("symbioticCore", "settlement")),
            "destination settlement",
        )
        .unwrap();

        assert_eq!(
            resolved.as_deref(),
            Some("0x2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn resolve_falls_back_to_predeploys() {
        let deployments: DeploymentsConfig =
            serde_json::from_str(r#"{ "source": {}, "destination": {} }"#).unwrap();

        let resolved = resolve(
            &env_config(),
            &deployments,
            ChainRole::Destination,
            "relayInfra.settlement",
            Some(("symbioticCore", "settlement")),
            "destination settlement",
        )
        .unwrap();

        assert_eq!(
            resolved.as_deref(),
            Some("0x2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn resolve_fails_on_drift() {
        let deployments: DeploymentsConfig = serde_json::from_str(
            r#"{
                "source": {},
                "destination": {
                    "relayInfra": {
                        "settlement": "0x1111111111111111111111111111111111111111"
                    }
                }
            }"#,
        )
        .unwrap();

        let error = resolve(
            &env_config(),
            &deployments,
            ChainRole::Destination,
            "relayInfra.settlement",
            Some(("symbioticCore", "settlement")),
            "destination settlement",
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("destination settlement drift: deployments has 0x1111111111111111111111111111111111111111, predeploys has 0x2222222222222222222222222222222222222222")
        );
    }
}
