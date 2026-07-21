use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub fn get(project_root: &Path, env_name: &str, key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| read_file(project_root, env_name).remove(key))
}

/// Check whether the environment-specific dotenv file exists.
pub fn env_file_exists(project_root: &Path, env_name: &str) -> bool {
    env_file_path(project_root, env_name).exists()
}

pub fn env_file_path(project_root: &Path, env_name: &str) -> std::path::PathBuf {
    project_root.join(format!(".env.{env_name}"))
}

/// All KEY=VALUE pairs from the environment-specific dotenv file (`.env.<env>`),
/// or empty if it doesn't exist. Used to snapshot the full set of variables
/// docker compose needs for interpolation, not just a single lookup.
pub fn read_all(project_root: &Path, env_name: &str) -> HashMap<String, String> {
    read_file(project_root, env_name)
}

fn read_file(project_root: &Path, env_name: &str) -> HashMap<String, String> {
    let path = env_file_path(project_root, env_name);
    let Ok(body) = fs::read_to_string(path) else {
        return HashMap::new();
    };

    let mut values = HashMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(name.trim().to_string(), value.trim().to_string());
    }
    values
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn falls_back_to_dotenv_file() {
        let temp_dir = tempdir().unwrap();
        fs::write(temp_dir.path().join(".env.local"), "FOO=bar\nEMPTY=\n").unwrap();

        assert_eq!(get(temp_dir.path(), "local", "FOO").as_deref(), Some("bar"));
        assert_eq!(
            get(temp_dir.path(), "local", "EMPTY").as_deref(),
            Some("")
        );
        assert_eq!(get(temp_dir.path(), "local", "MISSING"), None);
    }
}
