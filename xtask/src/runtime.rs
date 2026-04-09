use crate::config::EnvironmentConfig;
use crate::context::ResolvedContext;
use crate::envfile;

#[derive(Debug, Clone)]
pub struct RuntimeInputs {
    pub source_rpc: Option<String>,
    pub dest_rpc: Option<String>,
    pub private_key: Option<String>,
}

impl RuntimeInputs {
    pub fn resolve(context: &ResolvedContext, env_config: &EnvironmentConfig) -> Self {
        let private_key = env_config
            .deployer_signer(&context.project_root, &context.env_name)
            .map(|s| s.private_key)
            .ok();

        if env_config.is_local() {
            return Self {
                source_rpc: Some("http://localhost:8545".to_string()),
                dest_rpc: Some("http://localhost:8546".to_string()),
                private_key,
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
            private_key,
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
            failures.push("deployer signer is not configured".to_string());
        }
    }
}

pub fn setting(context: &ResolvedContext, key: &str) -> Option<String> {
    envfile::get(&context.project_root, &context.env_name, key)
}

#[cfg(test)]
pub fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
