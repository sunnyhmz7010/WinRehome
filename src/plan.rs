use crate::models::{
    ExclusionRule, InstalledAppRecord, PathStats, PortableAppCandidate, PortableConfidence,
    UserDataRoot,
};
use anyhow::Context;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use winreg::RegKey;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};

#[derive(Debug, Clone)]
pub struct BackupPreview {
    pub installed_apps: Vec<InstalledAppRecord>,
    pub portable_candidates: Vec<PortableAppCandidate>,
    pub user_data_roots: Vec<UserDataRoot>,
    pub exclusion_rules: Vec<ExclusionRule>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionSummary {
    pub selected_user_roots: usize,
    pub selected_portable_apps: usize,
    pub total_files: u64,
    pub total_bytes: u64,
}

impl BackupPreview {
    pub fn default_user_root_keys(&self) -> HashSet<String> {
        self.user_data_roots
            .iter()
            .filter(|root| root.default_selected)
            .map(|root| path_key(&root.path))
            .collect()
    }

    pub fn default_portable_keys(&self) -> HashSet<String> {
        self.portable_candidates
            .iter()
            .filter(|candidate| candidate.default_selected)
            .map(|candidate| path_key(&candidate.root_path))
            .collect()
    }

    pub fn summarize_selection(
        &self,
        selected_user_roots: &HashSet<String>,
        selected_portable_apps: &HashSet<String>,
    ) -> SelectionSummary {
        let mut summary = SelectionSummary::default();

        for root in &self.user_data_roots {
            if selected_user_roots.contains(&path_key(&root.path)) {
                summary.selected_user_roots += 1;
                summary.total_files += root.stats.file_count;
                summary.total_bytes += root.stats.total_bytes;
            }
        }

        for candidate in &self.portable_candidates {
            if selected_portable_apps.contains(&path_key(&candidate.root_path)) {
                summary.selected_portable_apps += 1;
                summary.total_files += candidate.stats.file_count;
                summary.total_bytes += candidate.stats.total_bytes;
            }
        }

        summary
    }
}

pub fn build_preview() -> anyhow::Result<BackupPreview> {
    let exclusion_rules = default_exclusion_rules();
    let installed_apps = scan_installed_apps()?;
    let user_data_roots = collect_user_data_roots(&exclusion_rules)?;
    let portable_candidates = scan_portable_candidates(&installed_apps)?;

    Ok(BackupPreview {
        installed_apps,
        portable_candidates,
        user_data_roots,
        exclusion_rules,
    })
}

fn scan_installed_apps() -> anyhow::Result<Vec<InstalledAppRecord>> {
    let hives = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY, "hklm-64"),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY, "hklm-32"),
        (HKEY_CURRENT_USER, KEY_READ | KEY_WOW64_64KEY, "hkcu-64"),
    ];

    let mut records = Vec::new();
    let mut seen = HashSet::new();

    for (hive, access, source) in hives {
        let root = RegKey::predef(hive);
        let uninstall = match root.open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            access,
        ) {
            Ok(key) => key,
            Err(_) => continue,
        };

        for key_name in uninstall.enum_keys().flatten() {
            let subkey = match uninstall.open_subkey_with_flags(&key_name, access) {
                Ok(value) => value,
                Err(_) => continue,
            };

            let display_name: String = match subkey.get_value::<String, _>("DisplayName") {
                Ok(value) if !value.trim().is_empty() => value,
                _ => continue,
            };

            let install_location = subkey
                .get_value::<String, _>("InstallLocation")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from);

            let dedupe_key = format!(
                "{}|{}",
                display_name.to_lowercase(),
                install_location
                    .as_ref()
                    .map(|path| path.display().to_string().to_lowercase())
                    .unwrap_or_default()
            );

            if seen.insert(dedupe_key) {
                records.push(InstalledAppRecord {
                    display_name,
                    source,
                    install_location,
                    uninstall_key: key_name,
                });
            }
        }
    }

    records.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    Ok(records)
}

fn collect_user_data_roots(exclusion_rules: &[ExclusionRule]) -> anyhow::Result<Vec<UserDataRoot>> {
    let profile = env::var_os("USERPROFILE").context("USERPROFILE is not available")?;
    let profile = PathBuf::from(profile);
    let roaming = profile.join("AppData\\Roaming");
    let local = profile.join("AppData\\Local");

    let candidates = [
        (
            "Personal Files",
            "Desktop",
            profile.join("Desktop"),
            "Files placed directly on the desktop often need migration.",
            true,
        ),
        (
            "Personal Files",
            "Documents",
            profile.join("Documents"),
            "Common personal documents and working files.",
            true,
        ),
        (
            "Personal Files",
            "Pictures",
            profile.join("Pictures"),
            "User photos and exported images.",
            true,
        ),
        (
            "Personal Files",
            "Videos",
            profile.join("Videos"),
            "Personal video files are often large, so keep them reviewable.",
            false,
        ),
        (
            "Personal Files",
            "Music",
            profile.join("Music"),
            "Media libraries can be large and are optional by default.",
            false,
        ),
        (
            "Personal Files",
            "Downloads",
            profile.join("Downloads"),
            "Downloads often contain installers and temporary files, so review before keeping.",
            false,
        ),
        (
            "Developer Settings",
            "SSH",
            profile.join(".ssh"),
            "SSH keys and config are high-value migration data.",
            true,
        ),
        (
            "Developer Settings",
            "GitConfig",
            profile.join(".gitconfig"),
            "Git identity and aliases are small but valuable.",
            true,
        ),
        (
            "App Settings",
            "VS Code User",
            roaming.join("Code\\User"),
            "Editor preferences and snippets migrate well.",
            true,
        ),
        (
            "App Settings",
            "Cursor User",
            roaming.join("Cursor\\User"),
            "Cursor settings and prompts are usually worth carrying over.",
            true,
        ),
        (
            "App Settings",
            "Windows Terminal",
            local.join("Packages\\Microsoft.WindowsTerminal_8wekyb3d8bbwe\\LocalState"),
            "Terminal profiles and settings are compact configuration data.",
            true,
        ),
        (
            "App Settings",
            "Chrome Bookmarks",
            local.join("Google\\Chrome\\User Data\\Default\\Bookmarks"),
            "Browser bookmarks are small and useful after reinstall.",
            true,
        ),
        (
            "App Settings",
            "Edge Bookmarks",
            local.join("Microsoft\\Edge\\User Data\\Default\\Bookmarks"),
            "Browser bookmarks are small and useful after reinstall.",
            true,
        ),
    ];

    let mut roots = Vec::new();
    for (category, label, path, reason, default_selected) in candidates {
        if path.exists() {
            let stats = estimate_path_stats(&path, exclusion_rules);
            roots.push(UserDataRoot {
                category,
                label,
                path,
                reason,
                default_selected,
                stats,
            });
        }
    }

    Ok(roots)
}

fn default_exclusion_rules() -> Vec<ExclusionRule> {
    vec![
        ExclusionRule {
            label: "System temp",
            pattern: "AppData\\Local\\Temp; Temp; Tmp",
        },
        ExclusionRule {
            label: "Browser cache",
            pattern: "Cache; Code Cache; GPUCache",
        },
        ExclusionRule {
            label: "Logs",
            pattern: "Logs; log",
        },
        ExclusionRule {
            label: "Node modules",
            pattern: "node_modules",
        },
        ExclusionRule {
            label: "Build outputs",
            pattern: "target; bin; obj; dist; build",
        },
    ]
}

fn scan_portable_candidates(
    installed_apps: &[InstalledAppRecord],
) -> anyhow::Result<Vec<PortableAppCandidate>> {
    let roots = discover_portable_search_roots();
    let installed_locations: Vec<PathBuf> = installed_apps
        .iter()
        .filter_map(|app| app.install_location.clone())
        .collect();

    let mut candidates = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }

        for entry in WalkDir::new(&root)
            .min_depth(1)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_dir() {
                continue;
            }

            let path = entry.path();
            if is_installed_location(path, &installed_locations) || is_known_noise(path) {
                continue;
            }

            if let Some(candidate) =
                evaluate_portable_directory(path, installed_locations.as_slice())
            {
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    candidates.truncate(100);
    Ok(candidates)
}

fn discover_portable_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(profile) = env::var_os("USERPROFILE") {
        let profile = PathBuf::from(profile);
        roots.push(profile.join("Desktop"));
        roots.push(profile.join("Downloads"));
        roots.push(profile.join("Tools"));
        roots.push(profile.join("PortableApps"));
    }

    for drive in ["D:\\", "E:\\", "F:\\"] {
        let base = PathBuf::from(drive);
        roots.push(base.join("Tools"));
        roots.push(base.join("PortableApps"));
        roots.push(base.join("Apps"));
    }

    roots
}

fn is_installed_location(path: &Path, installed_locations: &[PathBuf]) -> bool {
    is_system_install_path(path)
        || installed_locations.iter().any(|installed| {
            path == installed || path.starts_with(installed) || installed.starts_with(path)
        })
}

fn is_known_noise(path: &Path) -> bool {
    let lower = path.display().to_string().to_lowercase();
    lower.contains("\\appdata\\local\\temp")
        || lower.contains("\\cache\\")
        || lower.contains("\\code cache\\")
        || lower.contains("\\gpucache\\")
        || lower.contains("\\logs\\")
        || has_component(path, "temp")
        || has_component(path, "tmp")
        || has_component(path, "cache")
        || has_component(path, "logs")
        || has_component(path, "node_modules")
        || has_component(path, "target")
        || has_component(path, "bin")
        || has_component(path, "obj")
        || has_component(path, "dist")
        || has_component(path, "build")
}

fn evaluate_portable_directory(
    path: &Path,
    installed_locations: &[PathBuf],
) -> Option<PortableAppCandidate> {
    let mut executables = Vec::new();
    let mut support_file_hits = 0;
    let mut data_file_hits = 0;

    for entry in WalkDir::new(path)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();
        let extension = file_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());

        match extension.as_deref() {
            Some("exe") => executables.push(file_path.to_path_buf()),
            Some("dll" | "ini" | "json" | "yaml" | "toml" | "db") => support_file_hits += 1,
            Some("dat" | "sqlite" | "xml") => data_file_hits += 1,
            _ => {}
        }
    }

    if executables.is_empty() {
        return None;
    }

    let main_executable = executables
        .iter()
        .max_by_key(|path| score_executable_name(path))
        .cloned()?;
    let display_name = path.file_name()?.to_string_lossy().to_string();

    let confidence = if support_file_hits >= 3 && executables.len() <= 4 {
        PortableConfidence::High
    } else if support_file_hits + data_file_hits >= 1 {
        PortableConfidence::Medium
    } else {
        PortableConfidence::Low
    };

    let mut reasons = Vec::new();
    reasons.push(format!("{} executable(s) found", executables.len()));
    if support_file_hits > 0 {
        reasons.push(format!("{support_file_hits} support/config file(s) found"));
    }
    if data_file_hits > 0 {
        reasons.push(format!("{data_file_hits} portable data file(s) found"));
    }
    if is_system_install_path(path) || is_installed_location(path, installed_locations) {
        return None;
    }

    let stats = estimate_path_stats(path, &default_exclusion_rules());
    let default_selected = matches!(confidence, PortableConfidence::High);

    Some(PortableAppCandidate {
        display_name,
        root_path: path.to_path_buf(),
        main_executable,
        confidence,
        default_selected,
        stats,
        reasons,
    })
}

fn score_executable_name(path: &Path) -> usize {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let mut score = 0;
    if !file_name.contains("setup") && !file_name.contains("uninstall") {
        score += 5;
    }
    if !file_name.contains("helper") && !file_name.contains("update") {
        score += 3;
    }
    score + file_name.len()
}

fn estimate_path_stats(path: &Path, exclusion_rules: &[ExclusionRule]) -> PathStats {
    if !path.exists() {
        return PathStats::default();
    }

    if path.is_file() {
        return fs::metadata(path)
            .map(|metadata| PathStats {
                file_count: 1,
                total_bytes: metadata.len(),
            })
            .unwrap_or_default();
    }

    let mut stats = PathStats::default();
    let mut iterator = WalkDir::new(path).into_iter();
    while let Some(entry) = iterator.next() {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };

        if entry.depth() > 0 && should_exclude_path(entry.path(), exclusion_rules) {
            if entry.file_type().is_dir() {
                iterator.skip_current_dir();
            }
            continue;
        }

        if entry.file_type().is_file() {
            let file_stats = fs::metadata(entry.path())
                .map(|metadata| PathStats {
                    file_count: 1,
                    total_bytes: metadata.len(),
                })
                .unwrap_or_default();
            stats.add(file_stats);
        }
    }

    stats
}

fn should_exclude_path(path: &Path, _exclusion_rules: &[ExclusionRule]) -> bool {
    is_known_noise(path)
}

fn has_component(path: &Path, expected: &str) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|value| value.eq_ignore_ascii_case(expected))
            .unwrap_or(false)
    })
}

fn is_system_install_path(path: &Path) -> bool {
    path.starts_with("C:\\Program Files")
        || path.starts_with("C:\\Program Files (x86)")
        || path.starts_with("C:\\ProgramData\\Microsoft\\Windows\\Start Menu")
        || path.starts_with("C:\\Windows")
        || path.starts_with("C:\\Program Files\\WindowsApps")
}

pub fn path_key(path: &Path) -> String {
    path.display().to_string().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate_portable_directory, has_component, is_known_noise, is_system_install_path,
    };
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn treats_common_noise_directories_as_excluded() {
        assert!(is_known_noise(Path::new(
            "C:\\Users\\Sunny\\AppData\\Local\\Temp\\foo.txt"
        )));
        assert!(is_known_noise(Path::new(
            "D:\\PortableApps\\Tool\\node_modules\\left-pad\\index.js"
        )));
        assert!(!is_known_noise(Path::new(
            "D:\\PortableApps\\BinaryNinja\\plugins"
        )));
    }

    #[test]
    fn detects_system_install_paths() {
        assert!(is_system_install_path(Path::new(
            "C:\\Program Files\\Git\\bin\\git.exe"
        )));
        assert!(is_system_install_path(Path::new(
            "C:\\Windows\\System32\\notepad.exe"
        )));
        assert!(!is_system_install_path(Path::new(
            "D:\\PortableApps\\GitPortable"
        )));
    }

    #[test]
    fn portable_directory_gets_high_confidence_with_support_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-portable-{unique}"));
        fs::create_dir_all(&root).expect("create portable test root");
        fs::write(root.join("Tool.exe"), b"exe").expect("write exe");
        fs::write(root.join("tool.ini"), b"ini").expect("write ini");
        fs::write(root.join("tool.json"), b"json").expect("write json");
        fs::write(root.join("tool.db"), b"db").expect("write db");

        let candidate =
            evaluate_portable_directory(&root, &[]).expect("portable candidate expected");

        assert_eq!(candidate.confidence_label(), "high");
        assert!(candidate.default_selected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn component_matching_uses_exact_segments() {
        assert!(has_component(Path::new("D:\\foo\\bin\\app.exe"), "bin"));
        assert!(!has_component(Path::new("D:\\foo\\binary\\app.exe"), "bin"));
    }
}
