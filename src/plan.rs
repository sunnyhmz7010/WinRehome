use crate::models::{
    ExclusionRule, InstalledAppRecord, PortableAppCandidate, PortableConfidence, UserDataRoot,
};
use anyhow::Context;
use std::collections::HashSet;
use std::env;
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

pub fn build_preview() -> anyhow::Result<BackupPreview> {
    let installed_apps = scan_installed_apps()?;
    let user_data_roots = collect_user_data_roots()?;
    let exclusion_rules = default_exclusion_rules();
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

    let candidates = [
        ("Desktop", profile.join("Desktop")),
        ("Documents", profile.join("Documents")),
        ("Pictures", profile.join("Pictures")),
        ("Videos", profile.join("Videos")),
        ("Music", profile.join("Music")),
        ("Downloads", profile.join("Downloads")),
        ("RoamingConfig", profile.join("AppData\\Roaming")),
        ("SSH", profile.join(".ssh")),
        ("GitConfig", profile.join(".gitconfig")),
    ];

    let mut roots = Vec::new();
    for (label, path) in candidates {
        if path.exists() {
            roots.push(UserDataRoot { label, path });
        }
    }

    Ok(roots)
}

fn default_exclusion_rules() -> Vec<ExclusionRule> {
    vec![
        ExclusionRule {
            label: "System temp",
            pattern: "AppData\\Local\\Temp",
        },
        ExclusionRule {
            label: "Browser cache",
            pattern: "Cache",
        },
        ExclusionRule {
            label: "Logs",
            pattern: "Logs",
        },
        ExclusionRule {
            label: "Node modules",
            pattern: "node_modules",
        },
        ExclusionRule {
            label: "Build outputs",
            pattern: "target|bin|obj|dist|build",
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

            if let Some(candidate) = evaluate_portable_directory(path) {
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
    installed_locations.iter().any(|installed| {
        path == installed
            || path.starts_with(installed)
            || installed.starts_with(path)
            || path.starts_with("C:\\Program Files")
            || path.starts_with("C:\\Program Files (x86)")
    })
}

fn is_known_noise(path: &Path) -> bool {
    let lower = path.display().to_string().to_lowercase();
    lower.contains("\\cache")
        || lower.contains("\\temp")
        || lower.contains("\\logs")
        || lower.contains("\\node_modules")
        || lower.contains("\\target")
        || lower.contains("\\bin")
        || lower.contains("\\obj")
}

fn evaluate_portable_directory(path: &Path) -> Option<PortableAppCandidate> {
    let mut executables = Vec::new();
    let mut support_file_hits = 0;

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
    } else if support_file_hits >= 1 {
        PortableConfidence::Medium
    } else {
        PortableConfidence::Low
    };

    let mut reasons = Vec::new();
    reasons.push(format!("{} executable(s) found", executables.len()));
    if support_file_hits > 0 {
        reasons.push(format!("{support_file_hits} support/config file(s) found"));
    }

    Some(PortableAppCandidate {
        display_name,
        root_path: path.to_path_buf(),
        main_executable,
        confidence,
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
