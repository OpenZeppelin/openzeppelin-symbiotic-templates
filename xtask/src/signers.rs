use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use alloy::primitives::Address;
use eyre::{Result, bail, eyre};
use oz_keystore::LocalClient;

use crate::context::ResolvedContext;
use crate::envfile;
use crate::eth::{AlloyEth, EthApi};
use crate::runtime;
use crate::signer::encode_hex;
use crate::ui;

pub const RELAYER_SIGNER_COUNT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayerSigner {
    pub number: usize,
    pub id: String,
    pub keystore_path: PathBuf,
    pub address: Address,
}

/// Ensure relayer keystores exist, generating any that are missing.
///
/// If env-specific keystores exist at `config/keys/<env>/signer-{N}.json`,
/// copy them into the canonical location instead of generating fresh random ones.
pub fn ensure_relayer_keystores(project_root: &Path, env_name: &str) -> Result<()> {
    sync_env_keystores(project_root, env_name)?;

    let passphrase = relayer_passphrase(project_root, env_name);
    let all_exist = (0..RELAYER_SIGNER_COUNT)
        .all(|i| signer_keystore_path(project_root, env_name, i).is_file());

    if all_exist {
        return Ok(());
    }

    let step = ui::step("generate relayer keystores");
    generate_keystores(project_root, env_name, &passphrase)?;
    step.done("relayer keystores generated");
    Ok(())
}

/// Copy env-specific signer keystores (`config/keys/<env>/signer-{N}.json`)
/// into the canonical location (`config/keys/signer-{N}.json`) when present
/// and the canonical file is missing or stale.
fn sync_env_keystores(project_root: &Path, env_name: &str) -> Result<()> {
    let env_dir = project_root.join("config").join("keys").join(env_name);
    if !env_dir.is_dir() {
        return Ok(());
    }

    for index in 0..RELAYER_SIGNER_COUNT {
        let src = env_dir.join(format!("signer-{}.json", index + 1));
        if !src.is_file() {
            continue;
        }
        let dst = signer_keystore_path(project_root, index);
        if dst.is_file() {
            let src_bytes = fs::read(&src)?;
            let dst_bytes = fs::read(&dst)?;
            if src_bytes == dst_bytes {
                continue;
            }
        }
        fs::create_dir_all(dst.parent().ok_or_else(|| eyre!("no parent for {}", dst.display()))?)?;
        fs::copy(&src, &dst).map_err(|err| {
            eyre!(
                "failed to copy {} -> {}: {err}",
                src.display(),
                dst.display()
            )
        })?;
    }

    Ok(())
}

fn relayer_passphrase(project_root: &Path, env_name: &str) -> String {
    envfile::get(project_root, env_name, "KEYSTORE_PASSPHRASE")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "test-passphrase".to_string())
}

pub fn verify_signers(context: &ResolvedContext) -> Result<Vec<RelayerSigner>> {
    let signers = load_signers(context)?;
    for signer in &signers {
        ui::detail(&signer.id, format!("verified ({})", signer.address));
    }
    Ok(signers)
}

fn load_signers(context: &ResolvedContext) -> Result<Vec<RelayerSigner>> {
    let passphrase = passphrase_from_context(context)?;
    load_signers_with_passphrase(&context.project_root, &context.env_name, &passphrase)
}

pub fn signer_address_envs(context: &ResolvedContext) -> Result<Vec<(String, String)>> {
    Ok(load_signers(context)?
        .into_iter()
        .map(|signer| {
            (
                format!("SIGNER_{}_ADDRESS", signer.number),
                signer.address.to_string(),
            )
        })
        .collect())
}

pub fn passphrase_from_context(context: &ResolvedContext) -> Result<String> {
    runtime::setting(context, "KEYSTORE_PASSPHRASE")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre!("KEYSTORE_PASSPHRASE is not configured"))
}

pub fn signer_keystore_path(project_root: &Path, env_name: &str, index: usize) -> PathBuf {
    relayer_keystore_dir(project_root, env_name).join(format!("signer-{}.json", index + 1))
}

fn relayer_keystore_dir(project_root: &Path, env_name: &str) -> PathBuf {
    project_root.join("config").join("keys").join(env_name)
}

/// Generate random relayer signer keystores. Skips any that already exist.
pub fn generate_keystores(
    project_root: &Path,
    env_name: &str,
    passphrase: &str,
) -> Result<Vec<RelayerSigner>> {
    let keystore_dir = relayer_keystore_dir(project_root, env_name);
    fs::create_dir_all(&keystore_dir)?;

    let mut signers = Vec::with_capacity(RELAYER_SIGNER_COUNT);
    for index in 0..RELAYER_SIGNER_COUNT {
        let signer_name = format!("signer-{}", index + 1);
        let keystore_path = signer_keystore_path(project_root, env_name, index);

        if !keystore_path.exists() {
            generate_keystore(&keystore_dir, &signer_name, passphrase)?;
        }

        signers.push(load_signer_from_path(index, &keystore_path, passphrase)?);
    }

    Ok(signers)
}

pub fn load_signers_with_passphrase(
    project_root: &Path,
    env_name: &str,
    passphrase: &str,
) -> Result<Vec<RelayerSigner>> {
    (0..RELAYER_SIGNER_COUNT)
        .map(|index| {
            load_signer_from_path(
                index,
                &signer_keystore_path(project_root, env_name, index),
                passphrase,
            )
        })
        .collect()
}

fn load_signer_from_path(index: usize, path: &Path, passphrase: &str) -> Result<RelayerSigner> {
    if !path.is_file() {
        bail!(
            "relayer signer {} keystore not found at {}. Run `cargo xtask generate-signer --name signer-{}` to create.",
            index + 1,
            path.display(),
            index + 1
        );
    }

    let bytes = safe_local_client(
        format!("failed to load relayer signer {} keystore", index + 1),
        || LocalClient::load(path.to_path_buf(), passphrase.to_string()),
    )?;
    if bytes.len() != 32 {
        bail!(
            "relayer signer {} keystore yielded invalid private key length {}",
            index + 1,
            bytes.len()
        );
    }

    let key_hex = format!("0x{}", encode_hex(&bytes));
    let address = AlloyEth.address_from_private_key(&key_hex)?;

    Ok(RelayerSigner {
        number: index + 1,
        id: format!("signer-{}", index + 1),
        keystore_path: path.to_path_buf(),
        address,
    })
}

fn generate_keystore(dir: &Path, signer_name: &str, passphrase: &str) -> Result<()> {
    let filename = format!("{signer_name}.json");
    safe_local_client(
        format!("failed to generate keystore for {signer_name}"),
        || LocalClient::generate(dir.to_path_buf(), passphrase.to_string(), Some(&filename)),
    )?;
    Ok(())
}

#[cfg(test)]
pub fn write_keystore_from_private_key(
    dir: &Path,
    signer_name: &str,
    passphrase: &str,
    private_key: &str,
) -> Result<()> {
    let filename = format!("{signer_name}.json");
    let bytes = parse_private_key_bytes(private_key)?;
    safe_local_client(
        format!("failed to import keystore for {signer_name}"),
        || {
            LocalClient::update(
                dir.to_path_buf(),
                passphrase.to_string(),
                Some(&filename),
                &bytes,
            )
        },
    )?;
    Ok(())
}

#[cfg(test)]
fn parse_private_key_bytes(value: &str) -> Result<Vec<u8>> {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    if hex.len() != 64 {
        bail!("invalid private key length");
    }

    let mut bytes = Vec::with_capacity(32);
    for index in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[index..index + 2], 16)
            .map_err(|_| eyre!("invalid private key hex"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn safe_local_client<T>(context: String, operation: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(operation)).map_err(|_| eyre!("{context}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn generate_keystores_creates_missing_signers() {
        let temp_dir = tempdir().unwrap();
        let signers = generate_keystores(temp_dir.path(), "test", "test-passphrase").unwrap();

        assert_eq!(signers.len(), 3);
        for signer in &signers {
            assert!(signer.keystore_path.is_file());
        }

        // Running again should be idempotent (same signers)
        let signers2 = generate_keystores(temp_dir.path(), "test", "test-passphrase").unwrap();
        for (a, b) in signers.iter().zip(signers2.iter()) {
            assert_eq!(a.address, b.address);
        }
    }

    #[test]
    fn load_signers_with_passphrase_reports_missing_keystore() {
        let temp_dir = tempdir().unwrap();

        let err =
            load_signers_with_passphrase(temp_dir.path(), "test", "test-passphrase").unwrap_err();
        assert!(
            err.to_string()
                .contains("relayer signer 1 keystore not found")
        );
    }
}
