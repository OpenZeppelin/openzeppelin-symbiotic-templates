use std::env;
use std::path::{Path, PathBuf};

use eyre::{Result, eyre};

use crate::cli::GlobalArgs;

#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub project_root: PathBuf,
    pub env_name: String,
    pub env_config: PathBuf,
    pub deployments: PathBuf,
    pub generated_dir: PathBuf,
}

impl ResolvedContext {
    pub fn from_global(global: &GlobalArgs) -> Result<Self> {
        let env_name = global
            .env
            .clone()
            .or_else(|| env::var("ENV").ok())
            .unwrap_or_else(|| "local".to_string());

        Self::for_env_name(global, &env_name)
    }

    fn for_env_name(global: &GlobalArgs, env_name: &str) -> Result<Self> {
        let project_root = project_root()?;
        let env_config = resolve_path(
            &project_root,
            global
                .env_config
                .clone()
                .or_else(|| env::var_os("ENV_CONFIG").map(PathBuf::from))
                .unwrap_or_else(|| {
                    project_root
                        .join("config")
                        .join("environments")
                        .join(format!("{env_name}.json"))
                }),
        );
        let deployments = resolve_path(
            &project_root,
            global
                .deployments
                .clone()
                .or_else(|| env::var_os("DEPLOYMENTS_FILE").map(PathBuf::from))
                .unwrap_or_else(|| {
                    project_root
                        .join("deployments")
                        .join(format!("{env_name}.json"))
                }),
        );
        let generated_dir = resolve_path(
            &project_root,
            global
                .generated_dir
                .clone()
                .or_else(|| env::var_os("GENERATED_DIR").map(PathBuf::from))
                .unwrap_or_else(|| project_root.join("generated").join(env_name)),
        );

        Ok(Self {
            project_root,
            env_name: env_name.to_string(),
            env_config,
            deployments,
            generated_dir,
        })
    }
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
        eyre!(
            "failed to resolve project root from {}",
            manifest_dir.display()
        )
    })
}

fn resolve_path(project_root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_local_paths() {
        let context = ResolvedContext::from_global(&GlobalArgs::default()).unwrap();

        assert_eq!(context.env_name, "local");
        assert!(
            context
                .env_config
                .ends_with("config/environments/local.json")
        );
        assert!(context.deployments.ends_with("deployments/local.json"));
        assert!(context.generated_dir.ends_with("generated/local"));
    }
}
