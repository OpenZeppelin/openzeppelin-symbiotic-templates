use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use eyre::{Result, bail, eyre};
use oz_keystore::LocalClient;
use serde::Deserialize;

use crate::config::ConfigValue;

/// Anvil's 10 well-known private keys derived from
/// mnemonic "test test test test test test test test test test test junk"
/// via BIP-44 path m/44'/60'/0'/0/{index}.
const ANVIL_PRIVATE_KEYS: [&str; 10] = [
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
    "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
    "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97",
    "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6",
];

/// secp256k1 order n-1, used as the fixed P2P swarm key.
const SWARM_KEY: &str =
    "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignerConfig {
    Anvil(AnvilSignerConfig),
    Local(LocalSignerConfig),
    Env(EnvSignerConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnvilSignerConfig {
    pub index: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalSignerConfig {
    pub path: String,
    pub passphrase: ConfigValue,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvSignerConfig {
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedSigner {
    pub id: String,
    pub private_key: String,
    pub address: Address,
}

impl SignerConfig {
    pub fn resolve(
        &self,
        id: &str,
        project_root: &Path,
        env_name: &str,
    ) -> Result<ResolvedSigner> {
        let private_key = match self {
            Self::Anvil(config) => config.resolve()?,
            Self::Local(config) => config.resolve(project_root, env_name)?,
            Self::Env(config) => config.resolve(project_root, env_name)?,
        };

        let signer: PrivateKeySigner = private_key
            .parse()
            .map_err(|err| eyre!("signer '{id}': invalid private key: {err}"))?;

        Ok(ResolvedSigner {
            id: id.to_string(),
            private_key,
            address: signer.address(),
        })
    }
}

impl AnvilSignerConfig {
    fn resolve(&self) -> Result<String> {
        ANVIL_PRIVATE_KEYS
            .get(self.index)
            .map(|key| (*key).to_string())
            .ok_or_else(|| eyre!("anvil account index {} out of range (0-9)", self.index))
    }
}

impl LocalSignerConfig {
    fn resolve(&self, project_root: &Path, env_name: &str) -> Result<String> {
        let passphrase = self
            .passphrase
            .resolve(project_root, env_name)
            .ok_or_else(|| {
                eyre!(
                    "could not resolve passphrase for keystore '{}'",
                    self.path
                )
            })?;

        let keystore_path = project_root.join(&self.path);
        if !keystore_path.is_file() {
            bail!(
                "keystore not found at {}. Run `cargo xtask generate-signer` to create one.",
                keystore_path.display()
            );
        }

        let bytes = catch_keystore_panic(
            &format!("failed to decrypt keystore at {}", keystore_path.display()),
            || LocalClient::load(keystore_path.clone(), passphrase),
        )?;

        if bytes.len() != 32 {
            bail!(
                "keystore at {} yielded invalid key length {}",
                keystore_path.display(),
                bytes.len()
            );
        }

        Ok(format!("0x{}", encode_hex(&bytes)))
    }
}

impl EnvSignerConfig {
    fn resolve(&self, project_root: &Path, env_name: &str) -> Result<String> {
        crate::envfile::get(project_root, env_name, &self.value)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| eyre!("env var '{}' is not set or empty", self.value))
    }
}

/// Build the `--secret-keys` string for a relay sidecar from an operator's
/// EVM private key. Replicates the derivation previously done in
/// `start-sidecar.sh`.
pub fn build_sidecar_secret_keys(
    operator_key: &str,
    source_chain_id: u64,
    dest_chain_id: u64,
) -> String {
    let key_hex = operator_key.strip_prefix("0x").unwrap_or(operator_key);

    let secondary = derive_secondary_bls_key(key_hex);

    format!(
        "symb/0/15/{key_hex},\
         symb/0/11/{secondary},\
         symb/1/0/{key_hex},\
         evm/1/{source_chain_id}/{key_hex},\
         evm/1/{dest_chain_id}/{key_hex},\
         p2p/1/0/{SWARM_KEY},\
         p2p/1/1/{key_hex}"
    )
}

/// Derive secondary BLS key by adding 10000 to the low 32 bits of the key.
/// Matches the shell script: take last 8 hex chars, parse as u32, add 10000.
fn derive_secondary_bls_key(key_hex: &str) -> String {
    assert!(
        key_hex.len() >= 8,
        "BLS key too short ({} hex chars, need >= 8)",
        key_hex.len()
    );
    let prefix_len = key_hex.len() - 8;
    let prefix = &key_hex[..prefix_len];
    let last8 = &key_hex[prefix_len..];
    let low = u32::from_str_radix(last8, 16).unwrap_or(0);
    let new_low = low.wrapping_add(10000);
    format!("{prefix}{new_low:08x}")
}

/// Generate a new random keystore in the given directory.
pub fn generate_keystore(dir: &Path, name: &str, passphrase: &str) -> Result<ResolvedSigner> {
    std::fs::create_dir_all(dir)?;
    let filename = format!("{name}.json");
    let keystore_path = dir.join(&filename);

    if keystore_path.exists() {
        return load_keystore_signer(name, &keystore_path, passphrase);
    }

    catch_keystore_panic(
        &format!("failed to generate keystore for {name}"),
        || LocalClient::generate(dir.to_path_buf(), passphrase.to_string(), Some(&filename)),
    )?;

    load_keystore_signer(name, &keystore_path, passphrase)
}

fn load_keystore_signer(id: &str, path: &Path, passphrase: &str) -> Result<ResolvedSigner> {
    let bytes = catch_keystore_panic(
        &format!("failed to decrypt keystore at {}", path.display()),
        || LocalClient::load(path.to_path_buf(), passphrase.to_string()),
    )?;

    if bytes.len() != 32 {
        bail!(
            "keystore at {} yielded invalid key length {}",
            path.display(),
            bytes.len()
        );
    }

    let key_hex = format!("0x{}", encode_hex(&bytes));
    let signer: PrivateKeySigner = key_hex
        .parse()
        .map_err(|err| eyre!("keystore at {}: invalid key: {err}", path.display()))?;

    Ok(ResolvedSigner {
        id: id.to_string(),
        private_key: key_hex,
        address: signer.address(),
    })
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
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

fn catch_keystore_panic<T>(context: &str, f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        let detail = panic
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown error");
        eyre!("{context}: {detail}")
    })
}

/// Resolve a passphrase: CLI flag → interactive prompt (hidden input).
pub fn resolve_passphrase(flag: Option<&str>) -> Result<String> {
    if let Some(value) = flag {
        return Ok(value.to_string());
    }

    prompt_passphrase()
}

fn prompt_passphrase() -> Result<String> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        bail!("passphrase required: use --passphrase or run interactively");
    }

    let pass = rpassword::prompt_password("Enter keystore passphrase: ")
        .map_err(|e| eyre!("failed to read passphrase: {e}"))?;

    if pass.is_empty() {
        bail!("passphrase cannot be empty");
    }

    let confirm = rpassword::prompt_password("Confirm passphrase: ")
        .map_err(|e| eyre!("failed to read passphrase confirmation: {e}"))?;

    if pass != confirm {
        bail!("passphrases do not match");
    }

    Ok(pass)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn anvil_index_0_matches_well_known_key() {
        let config = AnvilSignerConfig { index: 0 };
        let key = config.resolve().unwrap();
        assert_eq!(
            key,
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        );
    }

    #[test]
    fn anvil_index_out_of_range() {
        let config = AnvilSignerConfig { index: 10 };
        assert!(config.resolve().is_err());
    }

    #[test]
    fn anvil_signer_resolves_correct_address() {
        let config = SignerConfig::Anvil(AnvilSignerConfig { index: 0 });
        let resolved = config.resolve("deployer", Path::new("."), "local").unwrap();
        assert_eq!(
            resolved.address,
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                .parse::<Address>()
                .unwrap()
        );
    }

    #[test]
    fn secondary_bls_key_derivation() {
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let secondary = derive_secondary_bls_key(key);
        // Last 8 hex = "f4f2ff80" = 0xf4f2ff80 = 4109238144
        // + 10000 = 4109248144 = 0xf4f32690
        assert_eq!(
            &secondary[secondary.len() - 8..],
            "f4f32690"
        );
        assert!(secondary.starts_with("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7b"));
    }

    #[test]
    fn build_sidecar_secret_keys_format() {
        let key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let result = build_sidecar_secret_keys(key, 31337, 31338);
        let parts: Vec<&str> = result.split(',').collect();
        assert_eq!(parts.len(), 7);
        assert!(parts[0].starts_with("symb/0/15/"));
        assert!(parts[1].starts_with("symb/0/11/"));
        assert!(parts[2].starts_with("symb/1/0/"));
        assert!(parts[3].starts_with("evm/1/31337/"));
        assert!(parts[4].starts_with("evm/1/31338/"));
        assert!(parts[5].starts_with("p2p/1/0/"));
        assert!(parts[6].starts_with("p2p/1/1/"));
    }

    #[test]
    fn generate_keystore_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().join("keys");
        let signer = generate_keystore(&dir, "test-signer", "test-pass").unwrap();
        assert!(dir.join("test-signer.json").exists());
        assert!(!signer.private_key.is_empty());

        // Idempotent: second call returns same address
        let signer2 = generate_keystore(&dir, "test-signer", "test-pass").unwrap();
        assert_eq!(signer.address, signer2.address);
    }

    #[test]
    fn env_signer_resolves_from_env_var() {
        let temp_dir = tempfile::tempdir().unwrap();
        let env_file = temp_dir.path().join(".env.test");
        std::fs::write(&env_file, "MY_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80\n").unwrap();
        let config = SignerConfig::Env(EnvSignerConfig {
            value: "MY_KEY".to_string(),
        });
        let resolved = config.resolve("deployer", temp_dir.path(), "test").unwrap();
        assert_eq!(
            resolved.address,
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                .parse::<Address>()
                .unwrap()
        );
    }

    #[test]
    fn env_signer_missing_var_errors() {
        let config = EnvSignerConfig {
            value: "NONEXISTENT_XTASK_TEST_KEY".to_string(),
        };
        let err = config.resolve(Path::new("/tmp"), "test").unwrap_err();
        assert!(err.to_string().contains("NONEXISTENT_XTASK_TEST_KEY"));
    }

    #[test]
    fn local_signer_missing_keystore_errors() {
        let config = LocalSignerConfig {
            path: "nonexistent/key.json".to_string(),
            passphrase: ConfigValue::Plain("test".to_string()),
        };
        let err = config.resolve(Path::new("/tmp"), "test").unwrap_err();
        assert!(err.to_string().contains("keystore not found"));
    }
}
