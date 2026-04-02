use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::envfile;

pub const DEFAULT_ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Anvil HD wallet accounts 1-3, each pre-funded with 10k ETH.
pub const ANVIL_OPERATOR_PRIVATE_KEYS: [&str; 3] = [
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
];

#[derive(Debug, Clone)]
pub struct RuntimeInputs {
    pub source_rpc: Option<String>,
    pub dest_rpc: Option<String>,
    pub private_key: Option<String>,
}

impl RuntimeInputs {
    pub fn resolve(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Self {
        if env_config.is_local() {
            return Self {
                source_rpc: Some("http://localhost:8545".to_string()),
                dest_rpc: Some("http://localhost:8546".to_string()),
                private_key: Some(DEFAULT_ANVIL_PRIVATE_KEY.to_string()),
            };
        }

        Self {
            source_rpc: env_config
                .chains
                .source
                .resolve_rpc_url(&context.project_root, &context.env_name)
                .or_else(|| {
                    envfile::get(&context.project_root, &context.env_name, "SOURCE_RPC_URL")
                }),
            dest_rpc: env_config
                .chains
                .destination
                .resolve_rpc_url(&context.project_root, &context.env_name)
                .or_else(|| {
                    envfile::get(&context.project_root, &context.env_name, "DEST_RPC_URL")
                }),
            private_key: envfile::get(&context.project_root, &context.env_name, "PRIVATE_KEY"),
        }
    }

    pub fn validate_non_local_presence(&self, failures: &mut Vec<String>) {
        if self.source_rpc.as_deref().unwrap_or_default().is_empty() {
            failures.push("SOURCE RPC is not configured".to_string());
        }
        if self.dest_rpc.as_deref().unwrap_or_default().is_empty() {
            failures.push("DEST RPC is not configured".to_string());
        }
        if self.private_key.as_deref().unwrap_or_default().is_empty() {
            failures.push("PRIVATE_KEY is not configured".to_string());
        }
    }
}

pub fn setting(context: &ResolvedContext, key: &str) -> Option<String> {
    envfile::get(&context.project_root, &context.env_name, key)
}

pub fn operator_private_key(context: &ResolvedContext, index: usize) -> Option<String> {
    setting(context, &format!("OPERATOR_{}_PRIVATE_KEY", index + 1)).or_else(|| {
        ANVIL_OPERATOR_PRIVATE_KEYS
            .get(index)
            .map(|key| (*key).to_string())
    })
}

#[cfg(test)]
pub fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
