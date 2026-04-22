use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub selected_user_roots: HashSet<String>,
    pub selected_portable_apps: HashSet<String>,
    pub last_archive_path: Option<String>,
    pub last_restore_destination: Option<String>,
    pub restore_user_data: bool,
    pub restore_portable_apps: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            selected_user_roots: HashSet::new(),
            selected_portable_apps: HashSet::new(),
            last_archive_path: None,
            last_restore_destination: None,
            restore_user_data: true,
            restore_portable_apps: true,
        }
    }
}

pub fn load_config() -> anyhow::Result<Option<AppConfig>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let bytes =
        fs::read(&path).with_context(|| format!("failed to read config {}", path.display()))?;
    let config: AppConfig =
        serde_json::from_slice(&bytes).context("failed to parse WinRehome config")?;
    Ok(Some(config))
}

pub fn save_config(config: &AppConfig) -> anyhow::Result<PathBuf> {
    let path = config_path()?;
    let parent = path
        .parent()
        .context("config path does not have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config dir {}", parent.display()))?;

    let bytes = serde_json::to_vec_pretty(config)?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    if let Some(config_dir) = dirs::config_dir() {
        return Ok(config_dir.join("WinRehome").join("config.json"));
    }

    if let Some(home_dir) = dirs::home_dir() {
        return Ok(home_dir.join(".winrehome").join("config.json"));
    }

    bail!("unable to determine a config directory for WinRehome")
}

pub fn normalize_existing_paths(paths: &HashSet<String>) -> HashSet<String> {
    paths
        .iter()
        .filter(|value| Path::new(value.as_str()).exists())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, normalize_existing_paths};
    use std::collections::HashSet;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn keeps_only_existing_paths() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let existing = std::env::temp_dir().join(format!("winrehome-config-{unique}"));
        fs::create_dir_all(&existing).expect("create existing dir");

        let paths = HashSet::from([
            existing.display().to_string(),
            "Z:\\definitely-missing-path".to_string(),
        ]);
        let normalized = normalize_existing_paths(&paths);

        assert_eq!(normalized.len(), 1);
        assert!(normalized.contains(&existing.display().to_string()));

        let _ = fs::remove_dir_all(existing);
    }

    #[test]
    fn app_config_defaults_are_empty() {
        let config = AppConfig::default();
        assert!(config.selected_user_roots.is_empty());
        assert!(config.selected_portable_apps.is_empty());
        assert!(config.last_archive_path.is_none());
        assert!(config.last_restore_destination.is_none());
        assert!(config.restore_user_data);
        assert!(config.restore_portable_apps);
    }
}
