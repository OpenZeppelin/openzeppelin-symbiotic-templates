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
use crate::ui;

pub const RELAYER_SIGNER_COUNT: usize = 3;
pub const MIN_RELAYER_NATIVE_BALANCE_WEI: u128 = 10_000_000_000_000_000;
pub const LEGACY_PUBLIC_LOCAL_RELAYER_ADDRESSES: [&str; RELAYER_SIGNER_COUNT] = [
    "0x976EA74026E726554dB657fA54763abd0C3a0aa9",
    "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955",
    "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayerSigner {
    pub number: usize,
    pub id: String,
    pub keystore_path: PathBuf,
    pub address: Address,
}

pub fn run_bootstrap_command(project_root: &Path, env_name: &str) -> Result<()> {
    let passphrase = envfile::get(project_root, env_name, "KEYSTORE_PASSPHRASE")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "test-passphrase".to_string());
    let signers = generate_keystores(project_root, &passphrase)?;

    for signer in signers {
        ui::detail(&signer.id, format!("ready ({})", signer.address));
    }

    Ok(())
}

pub fn verify_signers(context: &ResolvedContext) -> Result<Vec<RelayerSigner>> {
    let signers = load_signers(context)?;
    for signer in &signers {
        ui::detail(&signer.id, format!("verified ({})", signer.address));
    }
    Ok(signers)
}

pub fn load_signers(context: &ResolvedContext) -> Result<Vec<RelayerSigner>> {
    let passphrase = passphrase_from_context(context)?;
    load_signers_with_passphrase(&context.project_root, &passphrase)
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

pub fn signer_keystore_path(project_root: &Path, index: usize) -> PathBuf {
    project_root
        .join("config")
        .join("oz-relayer")
        .join("keys")
        .join(format!("signer-{}.json", index + 1))
}

pub fn is_legacy_public_local_signer(address: Address) -> bool {
    LEGACY_PUBLIC_LOCAL_RELAYER_ADDRESSES
        .iter()
        .filter_map(|value| value.parse::<Address>().ok())
        .any(|legacy| legacy == address)
}

/// Generate random relayer signer keystores. Skips any that already exist.
pub fn generate_keystores(project_root: &Path, passphrase: &str) -> Result<Vec<RelayerSigner>> {
    let keystore_dir = project_root.join("config").join("oz-relayer").join("keys");
    fs::create_dir_all(&keystore_dir)?;

    let mut signers = Vec::with_capacity(RELAYER_SIGNER_COUNT);
    for index in 0..RELAYER_SIGNER_COUNT {
        let signer_name = format!("signer-{}", index + 1);
        let keystore_path = signer_keystore_path(project_root, index);

        if !keystore_path.exists() {
            generate_keystore(&keystore_dir, &signer_name, passphrase)?;
        }

        signers.push(load_signer_from_path(index, &keystore_path, passphrase)?);
    }

    Ok(signers)
}

pub fn load_signers_with_passphrase(
    project_root: &Path,
    passphrase: &str,
) -> Result<Vec<RelayerSigner>> {
    (0..RELAYER_SIGNER_COUNT)
        .map(|index| {
            load_signer_from_path(
                index,
                &signer_keystore_path(project_root, index),
                passphrase,
            )
        })
        .collect()
}

fn load_signer_from_path(index: usize, path: &Path, passphrase: &str) -> Result<RelayerSigner> {
    if !path.is_file() {
        bail!(
            "relayer signer {} keystore not found at {}. Run `cargo xtask bootstrap-relayer-signers` to generate.",
            index + 1,
            path.display()
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

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(nibble_to_hex(byte >> 4));
        out.push(nibble_to_hex(byte & 0x0f));
    }
    out
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => unreachable!(),
    }
}

fn safe_local_client<T>(context: String, operation: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(operation)).map_err(|_| eyre!("{context}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::env;
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn generate_keystores_creates_missing_signers() {
        let temp_dir = tempdir().unwrap();
        let signers = generate_keystores(temp_dir.path(), "test-passphrase").unwrap();

        assert_eq!(signers.len(), 3);
        for signer in &signers {
            assert!(signer.keystore_path.is_file());
        }

        // Running again should be idempotent (same signers)
        let signers2 = generate_keystores(temp_dir.path(), "test-passphrase").unwrap();
        for (a, b) in signers.iter().zip(signers2.iter()) {
            assert_eq!(a.address, b.address);
        }
    }

    #[test]
    fn load_signers_with_passphrase_reports_missing_keystore() {
        let temp_dir = tempdir().unwrap();
        fs::create_dir_all(
            temp_dir
                .path()
                .join("config")
                .join("oz-relayer")
                .join("keys"),
        )
        .unwrap();

        let err = load_signers_with_passphrase(temp_dir.path(), "test-passphrase").unwrap_err();
        assert!(
            err.to_string()
                .contains("relayer signer 1 keystore not found")
        );
    }
}
