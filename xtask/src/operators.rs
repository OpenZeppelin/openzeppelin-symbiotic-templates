use std::fs;

use eyre::Result;
use tempfile::tempdir;

use crate::context::ResolvedContext;
use crate::eth::{AlloyEth, EthApi};
use crate::runner::{CommandRunner, CommandSpec};
use crate::runtime;

pub fn align_relayer_keystores<R: CommandRunner>(
    runner: &R,
    context: &ResolvedContext,
) -> Result<()> {
    let eth = AlloyEth;

    let Some(passphrase) =
        runtime::setting(context, "KEYSTORE_PASSPHRASE").filter(|value| !value.is_empty())
    else {
        eprintln!("WARNING: KEYSTORE_PASSPHRASE is not set; skipping relayer keystore alignment.");
        return Ok(());
    };

    let keystore_dir = context
        .project_root
        .join("config")
        .join("oz-relayer")
        .join("keys");
    fs::create_dir_all(&keystore_dir)?;

    println!("Aligning OZ relayer keystores with operator keys...");
    for index in 0..3 {
        let signer_name = format!("signer-{}", index + 1);
        let Some(private_key) =
            runtime::operator_private_key(context, index).filter(|value| !value.is_empty())
        else {
            eprintln!(
                "WARNING: OPERATOR_{}_PRIVATE_KEY is not set; skipping {}",
                index + 1,
                signer_name
            );
            continue;
        };

        let signer_addr = eth.address_from_private_key(&private_key).ok();

        let tmp_dir = tempdir()?;
        let import = runner.run(&CommandSpec::new(
            "cast",
            vec![
                "wallet".to_string(),
                "import".to_string(),
                "--keystore-dir".to_string(),
                tmp_dir.path().display().to_string(),
                "--private-key".to_string(),
                private_key,
                "--unsafe-password".to_string(),
                passphrase.clone(),
                signer_name.clone(),
            ],
        ))?;
        if !import.success {
            eprintln!("WARNING: Failed to generate keystore for {signer_name}");
            continue;
        }

        let signer_path = ["json", ""]
            .iter()
            .map(|suffix| {
                if suffix.is_empty() {
                    tmp_dir.path().join(&signer_name)
                } else {
                    tmp_dir.path().join(format!("{signer_name}.{suffix}"))
                }
            })
            .find(|path| path.is_file());

        let Some(signer_path) = signer_path else {
            eprintln!("WARNING: Keystore output missing for {signer_name}");
            continue;
        };

        fs::rename(signer_path, keystore_dir.join(format!("{signer_name}.json")))?;
        if let Some(address) = signer_addr {
            println!("        ✓ {signer_name} aligned ({address})");
        } else {
            println!("        ✓ {signer_name} aligned");
        }
    }

    Ok(())
}
