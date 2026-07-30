use std::env;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

use eyre::{Result, bail, eyre};

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

    /// Path to `contracts/deploy-data`. Env-scoped: this is managed as a
    /// symlink into `contracts/deploy-data-envs/<env>/` by
    /// [`ensure_deploy_data_env_link`].
    pub fn deploy_data_dir(&self) -> PathBuf {
        self.project_root.join("contracts").join("deploy-data")
    }
}

/// Ensures `contracts/deploy-data` is a symlink into this context's
/// `contracts/deploy-data-envs/<env>/` directory, so switching `ENV` can
/// never clobber another environment's deploy artifacts in place.
///
/// - If `contracts/deploy-data` doesn't exist, creates the env directory and
///   the symlink.
/// - If it's already a symlink pointing elsewhere, retargets it to the
///   current env's directory.
/// - If it's a plain directory (pre-env-scoping state), bails rather than
///   silently adopting its contents — they may belong to a different
///   environment than the one being invoked.
pub fn ensure_deploy_data_env_link(context: &ResolvedContext) -> Result<()> {
    let envs_root = context.project_root.join("contracts").join("deploy-data-envs");
    let env_dir = envs_root.join(&context.env_name);
    fs::create_dir_all(&env_dir)?;

    let link = context.deploy_data_dir();

    let metadata = match fs::symlink_metadata(&link) {
        Ok(metadata) => metadata,
        Err(_) => {
            unix_fs::symlink(&env_dir, &link)?;
            return Ok(());
        }
    };

    if !metadata.file_type().is_symlink() {
        bail!(
            "contracts/deploy-data is a plain directory from before env-scoping. Move it to contracts/deploy-data-envs/{}/ for whichever environment its artifacts belong to, then re-run.",
            context.env_name
        );
    }

    let current_target = fs::read_link(&link)?;
    let resolved_target = if current_target.is_absolute() {
        current_target
    } else {
        link.parent()
            .map(|parent| parent.join(&current_target))
            .unwrap_or(current_target)
    };

    if resolved_target != env_dir {
        fs::remove_file(&link)?;
        unix_fs::symlink(&env_dir, &link)?;
    }

    Ok(())
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
    use tempfile::tempdir;

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

    fn test_context(root: &Path, env_name: &str) -> ResolvedContext {
        ResolvedContext {
            project_root: root.to_path_buf(),
            env_name: env_name.to_string(),
            env_config: root.join("env.json"),
            deployments: root.join("deployments").join(format!("{env_name}.json")),
            generated_dir: root.join("generated").join(env_name),
        }
    }

    #[test]
    fn ensure_deploy_data_env_link_creates_symlink_when_absent() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("contracts")).unwrap();
        let context = test_context(root, "local-ccv");

        ensure_deploy_data_env_link(&context).unwrap();

        let link = context.deploy_data_dir();
        let metadata = fs::symlink_metadata(&link).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            root.join("contracts")
                .join("deploy-data-envs")
                .join("local-ccv")
        );
    }

    #[test]
    fn ensure_deploy_data_env_link_retargets_symlink_pointing_elsewhere() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("contracts")).unwrap();
        let envs_root = root.join("contracts").join("deploy-data-envs");
        let other_env_dir = envs_root.join("testnet-ccv");
        fs::create_dir_all(&other_env_dir).unwrap();
        fs::write(other_env_dir.join("marker.json"), "{}").unwrap();
        let link = root.join("contracts").join("deploy-data");
        unix_fs::symlink(&other_env_dir, &link).unwrap();

        let context = test_context(root, "local-ccv");
        ensure_deploy_data_env_link(&context).unwrap();

        assert_eq!(
            fs::read_link(&link).unwrap(),
            envs_root.join("local-ccv")
        );
        // The other environment's artifacts are untouched.
        assert!(other_env_dir.join("marker.json").exists());
    }

    #[test]
    fn ensure_deploy_data_env_link_bails_on_plain_directory() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let link = root.join("contracts").join("deploy-data");
        fs::create_dir_all(link.join("chainlink")).unwrap();

        let context = test_context(root, "local-ccv");
        let err = ensure_deploy_data_env_link(&context).unwrap_err();

        assert!(err.to_string().contains("plain directory"));
        // Untouched: still a plain directory, no symlink created.
        assert!(!fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }
}
