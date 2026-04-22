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
    let user_data_roots = collect_user_data_roots()?;
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

fn collect_user_data_roots() -> anyhow::Result<Vec<UserDataRoot>> {
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
            let stats = estimate_path_stats(&path);
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
    let mut seen_roots = HashSet::new();

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
            let path = entry.path();
            if is_installed_location(path, &installed_locations) || is_known_noise(path) {
                continue;
            }

            let candidate = if entry.file_type().is_dir() {
                evaluate_portable_directory(path, installed_locations.as_slice())
            } else if entry.file_type().is_file() && entry.depth() == 1 {
                evaluate_portable_executable(path, installed_locations.as_slice())
            } else {
                None
            };

            if let Some(candidate) = candidate {
                let root_key = path_key(&candidate.root_path);
                if seen_roots.insert(root_key) {
                    candidates.push(candidate);
                }
            }
        }
    }

    candidates.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    candidates.truncate(100);
    Ok(candidates)
}

fn discover_portable_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    if let Some(profile) = env::var_os("USERPROFILE") {
        let profile = PathBuf::from(profile);
        push_search_root(&mut roots, &mut seen, profile.join("Desktop"));
        push_search_root(&mut roots, &mut seen, profile.join("Downloads"));
        push_search_root(&mut roots, &mut seen, profile.join("Tools"));
        push_search_root(&mut roots, &mut seen, profile.join("PortableApps"));
    }

    for drive in existing_windows_drives() {
        push_search_root(&mut roots, &mut seen, drive.join("Tools"));
        push_search_root(&mut roots, &mut seen, drive.join("PortableApps"));
        push_search_root(&mut roots, &mut seen, drive.join("Apps"));
    }

    roots
}

fn push_search_root(roots: &mut Vec<PathBuf>, seen: &mut HashSet<String>, path: PathBuf) {
    let key = path_key(&path);
    if seen.insert(key) {
        roots.push(path);
    }
}

fn existing_windows_drives() -> Vec<PathBuf> {
    let mut drives = Vec::new();
    for letter in 'C'..='Z' {
        let drive = PathBuf::from(format!("{letter}:\\"));
        if drive.exists() {
            drives.push(drive);
        }
    }
    drives
}

fn is_installed_location(path: &Path, installed_locations: &[PathBuf]) -> bool {
    is_system_install_path(path)
        || installed_locations.iter().any(|installed| {
            path == installed || path.starts_with(installed) || installed.starts_with(path)
        })
}

fn is_known_noise(path: &Path) -> bool {
    let leaf = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    matches!(
        leaf.as_str(),
        "temp"
            | "tmp"
            | "cache"
            | "code cache"
            | "gpucache"
            | "logs"
            | "log"
            | "node_modules"
            | "target"
            | "bin"
            | "obj"
            | "dist"
            | "build"
    )
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

    let portable_executables: Vec<PathBuf> = executables
        .into_iter()
        .filter(|path| !is_probable_installer_executable(path))
        .collect();
    if portable_executables.is_empty() {
        return None;
    }

    let main_executable = portable_executables
        .iter()
        .max_by_key(|path| score_executable_name(path))
        .cloned()?;
    let display_name = path.file_name()?.to_string_lossy().to_string();

    let confidence = if support_file_hits >= 3 && portable_executables.len() <= 4 {
        PortableConfidence::High
    } else if support_file_hits + data_file_hits >= 1 {
        PortableConfidence::Medium
    } else {
        PortableConfidence::Low
    };

    let mut reasons = Vec::new();
    reasons.push(format!(
        "{} executable(s) found",
        portable_executables.len()
    ));
    if support_file_hits > 0 {
        reasons.push(format!("{support_file_hits} support/config file(s) found"));
    }
    if data_file_hits > 0 {
        reasons.push(format!("{data_file_hits} portable data file(s) found"));
    }
    if is_system_install_path(path) || is_installed_location(path, installed_locations) {
        return None;
    }

    let stats = estimate_path_stats(path);
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

fn evaluate_portable_executable(
    path: &Path,
    installed_locations: &[PathBuf],
) -> Option<PortableAppCandidate> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if extension.as_deref() != Some("exe") {
        return None;
    }
    if is_system_install_path(path)
        || is_installed_location(path, installed_locations)
        || is_probable_installer_executable(path)
    {
        return None;
    }

    let display_name = path.file_stem()?.to_string_lossy().trim().to_string();
    if display_name.is_empty() {
        return None;
    }

    let confidence = if looks_like_curated_portable_location(path) {
        PortableConfidence::High
    } else {
        PortableConfidence::Medium
    };
    let stats = estimate_path_stats(path);
    let default_selected = matches!(confidence, PortableConfidence::High);

    let mut reasons = vec!["single executable candidate found".to_string()];
    if looks_like_curated_portable_location(path) {
        reasons.push("stored under a common portable-app location".to_string());
    }

    Some(PortableAppCandidate {
        display_name,
        root_path: path.to_path_buf(),
        main_executable: path.to_path_buf(),
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

fn is_probable_installer_executable(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    [
        "setup",
        "install",
        "installer",
        "uninstall",
        "unins",
        "update",
        "updater",
        "patch",
    ]
    .iter()
    .any(|pattern| file_name.contains(pattern))
}

fn looks_like_curated_portable_location(path: &Path) -> bool {
    path.ancestors()
        .filter_map(|ancestor| ancestor.file_name())
        .filter_map(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .any(|segment| matches!(segment.as_str(), "portableapps" | "tools" | "apps"))
}

fn estimate_path_stats(path: &Path) -> PathStats {
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

        if entry.depth() > 0 && should_exclude_path(entry.path()) {
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

pub fn should_exclude_path(path: &Path) -> bool {
    is_known_noise(path)
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
        evaluate_portable_directory, evaluate_portable_executable, is_known_noise,
        is_system_install_path,
    };
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn treats_common_noise_directories_as_excluded() {
        assert!(is_known_noise(Path::new(
            "C:\\Users\\Sunny\\AppData\\Local\\Temp"
        )));
        assert!(is_known_noise(Path::new(
            "D:\\PortableApps\\Tool\\node_modules"
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
    fn detects_single_executable_candidate() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-single-exe-{unique}"));
        fs::create_dir_all(&root).expect("create single-exe test root");
        let exe = root.join("Tool.exe");
        fs::write(&exe, b"exe").expect("write exe");

        let candidate =
            evaluate_portable_executable(&exe, &[]).expect("single executable candidate expected");

        assert_eq!(candidate.display_name, "Tool");
        assert_eq!(candidate.root_path, exe);
        assert_eq!(candidate.main_executable, candidate.root_path);
        assert_eq!(candidate.stats.file_count, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_installer_like_executables() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-installer-exe-{unique}"));
        fs::create_dir_all(&root).expect("create installer-like test root");
        let installer = root.join("ToolSetup.exe");
        fs::write(&installer, b"exe").expect("write installer-like exe");

        assert!(evaluate_portable_executable(&installer, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_directory_with_only_setup_executable() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-setup-dir-{unique}"));
        fs::create_dir_all(&root).expect("create setup-only dir");
        fs::write(root.join("Setup.exe"), b"exe").expect("write setup exe");
        fs::write(root.join("tool.ini"), b"ini").expect("write support file");

        assert!(evaluate_portable_directory(&root, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn noise_matching_uses_leaf_name_only() {
        assert!(is_known_noise(Path::new("D:\\foo\\bin")));
        assert!(!is_known_noise(Path::new("D:\\foo\\binary\\app.exe")));
    }
}
