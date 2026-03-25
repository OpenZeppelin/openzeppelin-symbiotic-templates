use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::envfile;

pub const DEFAULT_ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

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
                .resolve_rpc_url(&context.project_root)
                .or_else(|| envfile::get(&context.project_root, "SOURCE_RPC_URL")),
            dest_rpc: env_config
                .chains
                .destination
                .resolve_rpc_url(&context.project_root)
                .or_else(|| envfile::get(&context.project_root, "DEST_RPC_URL")),
            private_key: envfile::get(&context.project_root, "PRIVATE_KEY"),
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
    envfile::get(&context.project_root, key)
}

pub fn operator_private_key(context: &ResolvedContext, index: usize) -> Option<String> {
    setting(context, &format!("OPERATOR_{}_PRIVATE_KEY", index + 1))
}

#[cfg(test)]
pub fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
