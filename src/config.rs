use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedScanRoot {
    pub path: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCustomUserRoot {
    pub path: String,
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl SavedWindowGeometry {
    pub fn is_valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub selected_user_roots: HashSet<String>,
    pub selected_portable_apps: HashSet<String>,
    pub selected_installed_app_dirs: HashSet<String>,
    pub scan_roots: Vec<SavedScanRoot>,
    pub excluded_scan_roots: Vec<SavedScanRoot>,
    pub custom_user_roots: Vec<SavedCustomUserRoot>,
    pub last_backup_output_dir: Option<String>,
    pub last_archive_path: Option<String>,
    pub last_restore_destination: Option<String>,
    pub restore_user_data: bool,
    pub restore_portable_apps: bool,
    pub restore_installed_app_dirs: bool,
    pub selected_restore_roots: HashSet<String>,
    pub skip_existing_restore_files: bool,
    pub remember_window_geometry: bool,
    pub last_window_geometry: Option<SavedWindowGeometry>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            selected_user_roots: HashSet::new(),
            selected_portable_apps: HashSet::new(),
            selected_installed_app_dirs: HashSet::new(),
            scan_roots: Vec::new(),
            excluded_scan_roots: Vec::new(),
            custom_user_roots: Vec::new(),
            last_backup_output_dir: None,
            last_archive_path: None,
            last_restore_destination: None,
            restore_user_data: true,
            restore_portable_apps: true,
            restore_installed_app_dirs: true,
            selected_restore_roots: HashSet::new(),
            skip_existing_restore_files: false,
            remember_window_geometry: true,
            last_window_geometry: None,
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

pub fn eframe_persistence_path() -> anyhow::Result<PathBuf> {
    let config_path = config_path()?;
    let parent = config_path
        .parent()
        .context("config path does not have a parent directory")?;
    Ok(parent.join("eframe-state.ron"))
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
        assert!(config.selected_installed_app_dirs.is_empty());
        assert!(config.scan_roots.is_empty());
        assert!(config.excluded_scan_roots.is_empty());
        assert!(config.custom_user_roots.is_empty());
        assert!(config.last_backup_output_dir.is_none());
        assert!(config.last_archive_path.is_none());
        assert!(config.last_restore_destination.is_none());
        assert!(config.restore_user_data);
        assert!(config.restore_portable_apps);
        assert!(config.restore_installed_app_dirs);
        assert!(config.selected_restore_roots.is_empty());
        assert!(!config.skip_existing_restore_files);
        assert!(config.remember_window_geometry);
        assert!(config.last_window_geometry.is_none());
    }
}
