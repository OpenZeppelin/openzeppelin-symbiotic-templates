use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub fn get(project_root: &Path, key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| read_file(project_root).remove(key))
}

fn read_file(project_root: &Path) -> HashMap<String, String> {
    let path = project_root.join(".env");
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
        fs::write(temp_dir.path().join(".env"), "FOO=bar\nEMPTY=\n").unwrap();

        assert_eq!(get(temp_dir.path(), "FOO").as_deref(), Some("bar"));
        assert_eq!(get(temp_dir.path(), "EMPTY").as_deref(), Some(""));
        assert_eq!(get(temp_dir.path(), "MISSING"), None);
    }
}
