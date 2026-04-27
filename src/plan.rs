use crate::models::{
    InstalledAppRecord, PathStats, PortableAppCandidate, PortableConfidence, UserDataRoot,
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
struct UserDataCandidateSpec {
    category: String,
    label: String,
    path: PathBuf,
    reason: String,
}

#[derive(Debug, Clone)]
struct DiscoveredUserDataCandidate {
    spec: UserDataCandidateSpec,
    score: usize,
}

#[derive(Debug, Clone)]
pub struct CustomUserDataRoot {
    pub path: PathBuf,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackupPreview {
    pub installed_apps: Vec<InstalledAppRecord>,
    pub portable_candidates: Vec<PortableAppCandidate>,
    pub user_data_roots: Vec<UserDataRoot>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionSummary {
    pub selected_user_roots: usize,
    pub selected_portable_apps: usize,
    pub selected_installed_app_dirs: usize,
    pub total_files: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub fraction: f32,
    pub stage: String,
    pub detail: String,
}

impl BackupPreview {
    pub fn summarize_selection(
        &self,
        selected_user_roots: &HashSet<String>,
        selected_portable_apps: &HashSet<String>,
        selected_installed_app_dirs: &HashSet<String>,
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

        for app in &self.installed_apps {
            if selected_installed_app_dirs.contains(&app.selection_key()) {
                if let Some(stats) = app.install_stats {
                    summary.selected_installed_app_dirs += 1;
                    summary.total_files += stats.file_count;
                    summary.total_bytes += stats.total_bytes;
                }
            }
        }

        summary
    }
}

pub fn build_preview_for_scan_roots_with_excludes_and_progress<F>(
    scan_roots: &[PathBuf],
    excluded_scan_roots: &[PathBuf],
    custom_user_roots: &[CustomUserDataRoot],
    mut on_progress: F,
) -> anyhow::Result<BackupPreview>
where
    F: FnMut(ScanProgress),
{
    on_progress(ScanProgress {
        fraction: 0.03,
        stage: "准备扫描".to_string(),
        detail: format!(
            "已确认 {} 个扫描根路径，{} 个排除路径",
            scan_roots.len(),
            excluded_scan_roots.len()
        ),
    });
    let installed_apps = scan_installed_apps_with_progress(|current, total, detail| {
        on_progress(ScanProgress {
            fraction: scale_progress(0.03, 0.48, current, total),
            stage: "读取安装软件".to_string(),
            detail,
        });
    });
    let installed_apps = installed_apps?;
    let user_data_roots = collect_user_data_roots_with_progress(
        scan_roots,
        excluded_scan_roots,
        custom_user_roots,
        |current, total, detail| {
            on_progress(ScanProgress {
                fraction: scale_progress(0.48, 0.78, current, total),
                stage: "收集个人文件".to_string(),
                detail,
            });
        },
    )?;
    let portable_candidates = scan_portable_candidates_with_progress(
        &installed_apps,
        scan_roots,
        excluded_scan_roots,
        |current, total, detail| {
            on_progress(ScanProgress {
                fraction: scale_progress(0.78, 0.96, current, total),
                stage: "扫描便携软件".to_string(),
                detail,
            });
        },
    )?;
    let user_data_roots =
        filter_user_data_roots_against_portable_candidates(user_data_roots, &portable_candidates);
    on_progress(ScanProgress {
        fraction: 0.98,
        stage: "整理结果".to_string(),
        detail: format!(
            "个人文件 {} 项，便携软件 {} 项，安装软件 {} 项",
            user_data_roots.len(),
            portable_candidates.len(),
            installed_apps.len()
        ),
    });

    let preview = BackupPreview {
        installed_apps,
        portable_candidates,
        user_data_roots,
    };
    on_progress(ScanProgress {
        fraction: 1.0,
        stage: "扫描完成".to_string(),
        detail: "正在整理扫描结果...".to_string(),
    });
    Ok(preview)
}

fn scan_installed_apps_with_progress<F>(
    mut on_progress: F,
) -> anyhow::Result<Vec<InstalledAppRecord>>
where
    F: FnMut(usize, usize, String),
{
    let hives = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY, "hklm-64"),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY, "hklm-32"),
        (HKEY_CURRENT_USER, KEY_READ | KEY_WOW64_64KEY, "hkcu-64"),
    ];

    let mut records = Vec::new();
    let mut seen = HashSet::new();
    let mut uninstall_sets = Vec::new();
    let mut total_keys = 0_usize;

    for (hive, access, source) in hives {
        let root = RegKey::predef(hive);
        let uninstall = match root.open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            access,
        ) {
            Ok(key) => key,
            Err(_) => continue,
        };
        let key_names: Vec<String> = uninstall.enum_keys().flatten().collect();
        total_keys += key_names.len();
        uninstall_sets.push((uninstall, key_names, access, source));
    }

    let mut processed_keys = 0_usize;
    on_progress(
        0,
        total_keys.max(1),
        "正在读取注册表软件清单...".to_string(),
    );

    for (uninstall, key_names, access, source) in uninstall_sets {
        for key_name in key_names {
            processed_keys += 1;
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
                let install_stats = install_location
                    .as_ref()
                    .filter(|path| path.exists())
                    .map(|path| estimate_path_stats(path));
                records.push(InstalledAppRecord {
                    display_name,
                    source,
                    install_location,
                    install_stats,
                    uninstall_key: key_name,
                });
            }

            let detail = format!(
                "已处理 {}/{} 个软件项，当前来源 {}，已识别 {} 条记录",
                processed_keys,
                total_keys.max(1),
                source,
                records.len()
            );
            on_progress(processed_keys, total_keys.max(1), detail);
        }
    }

    records.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    Ok(records)
}

fn collect_user_data_roots_with_progress<F>(
    scan_roots: &[PathBuf],
    excluded_scan_roots: &[PathBuf],
    custom_user_roots: &[CustomUserDataRoot],
    mut on_progress: F,
) -> anyhow::Result<Vec<UserDataRoot>>
where
    F: FnMut(usize, usize, String),
{
    let profile = env::var_os("USERPROFILE").context("USERPROFILE is not available")?;
    let profile = PathBuf::from(profile);
    let roaming = profile.join("AppData\\Roaming");
    let local = profile.join("AppData\\Local");
    let mut candidates = default_user_data_candidates(&profile, &roaming, &local);
    candidates.extend(discover_scan_root_user_data_candidates(
        scan_roots,
        excluded_scan_roots,
    ));
    candidates.extend(custom_user_data_candidates(custom_user_roots));
    let total_candidates = candidates.len().max(1);

    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for (index, candidate) in candidates.into_iter().enumerate() {
        let progress_label = candidate.label.clone();
        if candidate.path.exists() && seen.insert(path_key(&candidate.path)) {
            let stats = estimate_path_stats(&candidate.path);
            roots.push(UserDataRoot {
                category: candidate.category.into(),
                label: candidate.label.into(),
                path: candidate.path,
                reason: candidate.reason.into(),
                stats,
            });
        }
        on_progress(
            index + 1,
            total_candidates,
            format!(
                "已检查 {}/{} 个候选路径，当前项目 {}",
                index + 1,
                total_candidates,
                progress_label
            ),
        );
    }

    Ok(roots)
}

fn filter_user_data_roots_against_portable_candidates(
    roots: Vec<UserDataRoot>,
    portable_candidates: &[PortableAppCandidate],
) -> Vec<UserDataRoot> {
    roots
        .into_iter()
        .filter(|root| {
            !portable_candidates.iter().any(|candidate| {
                root.path == candidate.root_path || root.path.starts_with(&candidate.root_path)
            })
        })
        .collect()
}

fn discover_scan_root_user_data_candidates(
    scan_roots: &[PathBuf],
    excluded_scan_roots: &[PathBuf],
) -> Vec<UserDataCandidateSpec> {
    let excluded_keys: Vec<String> = excluded_scan_roots
        .iter()
        .map(|path| path_key(path))
        .collect();
    let mut discovered = Vec::new();
    let mut seen = HashSet::new();

    for root in scan_roots {
        if !root.exists() || !root.is_dir() || is_path_excluded(root, excluded_keys.as_slice()) {
            continue;
        }

        let mut walker = WalkDir::new(root).min_depth(1).max_depth(2).into_iter();
        while let Some(entry) = walker.next() {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            let dir_like = entry.file_type().is_dir()
                || (entry.file_type().is_symlink()
                    && fs::metadata(path)
                        .map(|metadata| metadata.is_dir())
                        .unwrap_or(false));
            let file_like = entry.file_type().is_file()
                || (entry.file_type().is_symlink()
                    && fs::metadata(path)
                        .map(|metadata| metadata.is_file())
                        .unwrap_or(false));

            if is_path_excluded(path, excluded_keys.as_slice()) {
                if dir_like {
                    walker.skip_current_dir();
                }
                continue;
            }
            if dir_like && should_skip_portable_search_dir(root, path) {
                walker.skip_current_dir();
                continue;
            }
            if is_known_noise(path) {
                if dir_like {
                    walker.skip_current_dir();
                }
                continue;
            }

            let candidate_key = path_key(path);
            if file_like {
                if entry.depth() == 1 {
                    if let Some(candidate) = classify_scan_root_user_file_candidate(path) {
                        if seen.insert(candidate_key) {
                            discovered.push(DiscoveredUserDataCandidate {
                                score: user_data_file_candidate_score(path),
                                spec: candidate,
                            });
                        }
                    }
                }
                continue;
            }
            if !dir_like {
                continue;
            }

            if let Some((candidate, score)) = evaluate_scan_root_user_directory_candidate(path) {
                if seen.insert(candidate_key) {
                    discovered.push(DiscoveredUserDataCandidate {
                        spec: candidate,
                        score,
                    });
                }
            }
        }
    }

    reconcile_discovered_user_data_candidates(discovered)
}

fn classify_scan_root_user_file_candidate(path: &Path) -> Option<UserDataCandidateSpec> {
    let kind = classify_user_data_file_kind(path)?;
    Some(UserDataCandidateSpec {
        category: user_data_category_for_kind(kind).to_string(),
        label: default_custom_user_data_label(path),
        path: path.to_path_buf(),
        reason: user_data_reason_for_kind(kind).to_string(),
    })
}

fn user_data_file_candidate_score(path: &Path) -> usize {
    let kind = classify_user_data_file_kind(path).unwrap_or(UserDataValueKind::Documents);
    let base = match kind {
        UserDataValueKind::DiskImages => 150,
        UserDataValueKind::DeveloperData => 130,
        UserDataValueKind::AppData => 120,
        UserDataValueKind::Archives => 110,
        UserDataValueKind::Media => 100,
        UserDataValueKind::Documents => 90,
    };
    let size_bonus = fs::metadata(path)
        .map(|metadata| (metadata.len() / (1024 * 1024)).min(50) as usize)
        .unwrap_or(0);
    base + size_bonus
}

#[cfg(test)]
fn classify_scan_root_user_directory_candidate(path: &Path) -> Option<UserDataCandidateSpec> {
    evaluate_scan_root_user_directory_candidate(path).map(|(candidate, _)| candidate)
}

fn evaluate_scan_root_user_directory_candidate(
    path: &Path,
) -> Option<(UserDataCandidateSpec, usize)> {
    let evidence = collect_user_data_directory_evidence(path);
    let kind = classify_user_data_directory_kind(path, &evidence)?;
    let score = user_data_directory_candidate_score(kind, path, &evidence);
    let candidate = UserDataCandidateSpec {
        category: user_data_category_for_kind(kind).to_string(),
        label: default_custom_user_data_label(path),
        path: path.to_path_buf(),
        reason: user_data_reason_for_kind(kind).to_string(),
    };
    Some((candidate, score))
}

fn user_data_directory_candidate_score(
    kind: UserDataValueKind,
    path: &Path,
    evidence: &UserDataDirectoryEvidence,
) -> usize {
    let base = match kind {
        UserDataValueKind::DiskImages => 160,
        UserDataValueKind::DeveloperData => 140,
        UserDataValueKind::AppData => 130,
        UserDataValueKind::Archives => 120,
        UserDataValueKind::Media => 110,
        UserDataValueKind::Documents => 100,
    };
    let evidence_bonus = evidence.document_files.min(12)
        + evidence.media_files.min(12)
        + evidence.archive_files.min(8) * 2
        + evidence.disk_image_files.min(8) * 4
        + evidence.notebook_files.min(8) * 5
        + evidence.app_data_files.min(12) * 3
        + evidence.android_avd_child_dirs.min(4) * 8
        + evidence.android_sdk_component_dirs.min(6) * 8
        + evidence.config_dir_hits.min(6) * 4;
    let generic_name_penalty = path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(is_generic_user_container_name) as usize
        * 20;

    base + evidence_bonus - generic_name_penalty
}

fn reconcile_discovered_user_data_candidates(
    mut candidates: Vec<DiscoveredUserDataCandidate>,
) -> Vec<UserDataCandidateSpec> {
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                right
                    .spec
                    .path
                    .components()
                    .count()
                    .cmp(&left.spec.path.components().count())
            })
            .then_with(|| left.spec.label.cmp(&right.spec.label))
    });

    let mut kept: Vec<DiscoveredUserDataCandidate> = Vec::new();
    for candidate in candidates {
        if kept
            .iter()
            .any(|existing| paths_overlap(&existing.spec.path, &candidate.spec.path))
        {
            continue;
        }
        kept.push(candidate);
    }

    kept.sort_by(|left, right| left.spec.label.cmp(&right.spec.label));
    kept.into_iter().map(|candidate| candidate.spec).collect()
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn collect_user_data_directory_evidence(path: &Path) -> UserDataDirectoryEvidence {
    let mut evidence = UserDataDirectoryEvidence::default();
    let directory_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    evidence.wallpaper_name_hint = directory_name.contains("wallpaper")
        || directory_name.contains("background")
        || directory_name.contains("壁纸");
    evidence.notebook_name_hint =
        directory_name.contains("jupyter") || directory_name.contains("notebook");

    let mut walker = WalkDir::new(path)
        .follow_links(true)
        .min_depth(1)
        .max_depth(4)
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let entry_path = entry.path();
        if should_exclude_path(entry_path) {
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }

        if entry.file_type().is_dir() {
            if entry.depth() == 1
                && entry_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(".avd"))
            {
                evidence.android_avd_child_dirs += 1;
            }
            if entry.depth() == 1
                && entry_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(is_android_sdk_component_directory_name)
            {
                evidence.android_sdk_component_dirs += 1;
            }
            if entry.depth() <= 2
                && entry_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(is_probable_app_data_directory_name)
            {
                evidence.config_dir_hits += 1;
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        evidence.total_files += 1;
        if is_executable_file(entry_path) {
            evidence.executable_files += 1;
        }
        if evidence.total_files >= 400 {
            break;
        }

        match classify_user_data_file_kind(entry_path) {
            Some(UserDataValueKind::Documents) => evidence.document_files += 1,
            Some(UserDataValueKind::Media) => evidence.media_files += 1,
            Some(UserDataValueKind::Archives) => evidence.archive_files += 1,
            Some(UserDataValueKind::DiskImages) => evidence.disk_image_files += 1,
            Some(UserDataValueKind::DeveloperData) => evidence.notebook_files += 1,
            Some(UserDataValueKind::AppData) => evidence.app_data_files += 1,
            None => {}
        }

        if entry_path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(is_android_avd_marker_file_name)
        {
            evidence.android_avd_marker_files += 1;
        }
    }

    evidence
}

fn classify_user_data_directory_kind(
    _path: &Path,
    evidence: &UserDataDirectoryEvidence,
) -> Option<UserDataValueKind> {
    if evidence.android_sdk_component_dirs >= 3 {
        return Some(UserDataValueKind::DeveloperData);
    }
    if evidence.android_avd_child_dirs >= 1
        || (evidence.android_avd_marker_files >= 2 && evidence.disk_image_files >= 1)
    {
        return Some(UserDataValueKind::DeveloperData);
    }
    if evidence.notebook_files >= 1 {
        return Some(UserDataValueKind::DeveloperData);
    }
    if evidence.disk_image_files >= 1 {
        return Some(UserDataValueKind::DiskImages);
    }
    if evidence.app_data_files >= 8 && evidence.executable_files == 0 {
        return Some(UserDataValueKind::AppData);
    }
    if evidence.config_dir_hits >= 2
        && evidence.app_data_files >= 2
        && evidence.executable_files == 0
    {
        return Some(UserDataValueKind::AppData);
    }
    if evidence.archive_files >= 3 && evidence.executable_files == 0 {
        return Some(UserDataValueKind::Archives);
    }
    if evidence.media_files >= 8 && evidence.executable_files == 0 {
        return Some(UserDataValueKind::Media);
    }
    if evidence.wallpaper_name_hint && evidence.media_files >= 1 && evidence.executable_files == 0 {
        return Some(UserDataValueKind::Media);
    }
    if evidence.document_files >= 5 && evidence.executable_files == 0 {
        return Some(UserDataValueKind::Documents);
    }
    if evidence.notebook_name_hint && evidence.document_files >= 2 && evidence.executable_files == 0
    {
        return Some(UserDataValueKind::DeveloperData);
    }

    None
}

fn classify_user_data_file_kind(path: &Path) -> Option<UserDataValueKind> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let kind = if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "pdf"
                | "txt"
                | "md"
                | "csv"
                | "rtf"
                | "xmind"
        )
    }) {
        UserDataValueKind::Documents
    } else if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "bmp"
                | "gif"
                | "tif"
                | "tiff"
                | "heic"
                | "psd"
                | "kra"
                | "mp4"
                | "mkv"
                | "mov"
                | "avi"
                | "wmv"
                | "mp3"
                | "flac"
                | "wav"
                | "m4a"
        )
    }) {
        UserDataValueKind::Media
    } else if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst"
        )
    }) {
        UserDataValueKind::Archives
    } else if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "qcow2" | "vdi" | "ova" | "ovf"
        )
    }) {
        UserDataValueKind::DiskImages
    } else if extension.as_deref() == Some("ipynb") {
        UserDataValueKind::DeveloperData
    } else if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "db" | "sqlite" | "sqlite3" | "dat" | "json" | "ini" | "cfg" | "wal" | "shm" | "xml"
        )
    }) || is_probable_app_data_file_name(&file_name)
    {
        UserDataValueKind::AppData
    } else {
        return None;
    };

    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.len() > 0)
        .map(|_| kind)
}

fn user_data_category_for_kind(kind: UserDataValueKind) -> &'static str {
    match kind {
        UserDataValueKind::Documents => "Personal Files",
        UserDataValueKind::Media => "Personal Media",
        UserDataValueKind::Archives => "Archives",
        UserDataValueKind::DiskImages => "Disk Images",
        UserDataValueKind::DeveloperData => "Developer Data",
        UserDataValueKind::AppData => "App Data",
    }
}

fn user_data_reason_for_kind(kind: UserDataValueKind) -> &'static str {
    match kind {
        UserDataValueKind::Documents => {
            "Document-heavy folders usually contain user-authored work worth migrating."
        }
        UserDataValueKind::Media => {
            "Media-heavy folders often contain personal assets that are expensive to rebuild."
        }
        UserDataValueKind::Archives => {
            "Archive-heavy folders may contain curated packages or backups worth preserving."
        }
        UserDataValueKind::DiskImages => {
            "Disk image and virtual-machine files are large but often intentional user assets."
        }
        UserDataValueKind::DeveloperData => {
            "Developer workspaces such as notebooks or emulator data can be costly to recreate."
        }
        UserDataValueKind::AppData => {
            "Application data folders often contain databases, configuration, and account state worth migrating."
        }
    }
}

fn is_android_avd_marker_file_name(file_name: &str) -> bool {
    let lowered = file_name.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "config.ini"
            | "hardware-qemu.ini"
            | "userdata-qemu.img"
            | "cache.img"
            | "sdcard.img"
            | "multiinstance.lock"
    )
}

fn is_android_sdk_component_directory_name(directory_name: &str) -> bool {
    let lowered = directory_name.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "platforms"
            | "platform-tools"
            | "build-tools"
            | "cmdline-tools"
            | "emulator"
            | "skins"
            | "sources"
            | "system-images"
            | "ndk"
            | "extras"
            | "licenses"
            | "patcher"
    )
}

fn is_generic_user_container_name(directory_name: &str) -> bool {
    let lowered = directory_name.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "files"
            | "data"
            | "backup"
            | "backups"
            | "work"
            | "workspace"
            | "images"
            | "media"
            | "misc"
            | "other"
            | "others"
    )
}

fn is_probable_app_data_directory_name(directory_name: &str) -> bool {
    let lowered = directory_name.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "config"
            | "configs"
            | "data"
            | "backup"
            | "backups"
            | "db"
            | "database"
            | "databases"
            | "sqlite"
            | "login"
            | "profiles"
            | "profile"
            | "head_imgs"
            | "finderlive"
            | "nt_db"
            | "nt_data"
    )
}

fn is_probable_app_data_file_name(file_name: &str) -> bool {
    let lowered = file_name.trim().to_ascii_lowercase();
    lowered.contains("config")
        || lowered.contains("session")
        || lowered.contains("key_info")
        || lowered.contains("backup")
        || matches!(
            lowered.as_str(),
            "global_config" | "client_config" | "lock.ini"
        )
}

fn default_user_data_candidates(
    profile: &Path,
    roaming: &Path,
    local: &Path,
) -> Vec<UserDataCandidateSpec> {
    let documents = profile.join("Documents");

    vec![
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Desktop".to_string(),
            path: profile.join("Desktop"),
            reason: "Files placed directly on the desktop often need migration.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Documents".to_string(),
            path: documents.clone(),
            reason: "Common personal documents and working files.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Pictures".to_string(),
            path: profile.join("Pictures"),
            reason: "User photos and exported images.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Videos".to_string(),
            path: profile.join("Videos"),
            reason: "Personal video files are often large, so keep them reviewable.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Music".to_string(),
            path: profile.join("Music"),
            reason: "Media libraries can be large and are optional by default.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Downloads".to_string(),
            path: profile.join("Downloads"),
            reason:
                "Downloads often contain installers and temporary files, so review before keeping."
                    .to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Favorites".to_string(),
            path: profile.join("Favorites"),
            reason: "Legacy browser and shell favorites are small and easy to carry over."
                .to_string(),
        },
        UserDataCandidateSpec {
            category: "Personal Files".to_string(),
            label: "Saved Games".to_string(),
            path: profile.join("Saved Games"),
            reason: "Many Windows games still keep local saves here.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Developer Settings".to_string(),
            label: "SSH".to_string(),
            path: profile.join(".ssh"),
            reason: "SSH keys and config are high-value migration data.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Developer Settings".to_string(),
            label: "GitConfig".to_string(),
            path: profile.join(".gitconfig"),
            reason: "Git identity and aliases are small but valuable.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Developer Settings".to_string(),
            label: "Windows PowerShell Profile".to_string(),
            path: documents.join("WindowsPowerShell\\Microsoft.PowerShell_profile.ps1"),
            reason: "Legacy PowerShell profile customizations are often hand-tuned.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Developer Settings".to_string(),
            label: "PowerShell Profile".to_string(),
            path: documents.join("PowerShell\\Microsoft.PowerShell_profile.ps1"),
            reason: "PowerShell 7 profile customizations are small and worth carrying over."
                .to_string(),
        },
        UserDataCandidateSpec {
            category: "App Settings".to_string(),
            label: "VS Code User".to_string(),
            path: roaming.join("Code\\User"),
            reason: "Editor preferences and snippets migrate well.".to_string(),
        },
        UserDataCandidateSpec {
            category: "App Settings".to_string(),
            label: "Cursor User".to_string(),
            path: roaming.join("Cursor\\User"),
            reason: "Cursor settings and prompts are usually worth carrying over.".to_string(),
        },
        UserDataCandidateSpec {
            category: "App Settings".to_string(),
            label: "Windows Terminal".to_string(),
            path: local.join("Packages\\Microsoft.WindowsTerminal_8wekyb3d8bbwe\\LocalState"),
            reason: "Terminal profiles and settings are compact configuration data.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Browser Data".to_string(),
            label: "Chrome Bookmarks".to_string(),
            path: local.join("Google\\Chrome\\User Data\\Default\\Bookmarks"),
            reason: "Browser bookmarks are small and useful after reinstall.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Browser Data".to_string(),
            label: "Edge Bookmarks".to_string(),
            path: local.join("Microsoft\\Edge\\User Data\\Default\\Bookmarks"),
            reason: "Browser bookmarks are small and useful after reinstall.".to_string(),
        },
        UserDataCandidateSpec {
            category: "Browser Data".to_string(),
            label: "Firefox Profiles".to_string(),
            path: roaming.join("Mozilla\\Firefox"),
            reason:
                "Firefox profiles preserve bookmarks, extensions metadata, and profile settings."
                    .to_string(),
        },
        UserDataCandidateSpec {
            category: "App Settings".to_string(),
            label: "Notepad++".to_string(),
            path: roaming.join("Notepad++"),
            reason: "Notepad++ preferences and session data are compact and migration-friendly."
                .to_string(),
        },
        UserDataCandidateSpec {
            category: "App Settings".to_string(),
            label: "OBS Studio".to_string(),
            path: roaming.join("obs-studio"),
            reason: "OBS scenes, profiles, and output settings can take time to rebuild."
                .to_string(),
        },
    ]
}

fn custom_user_data_candidates(
    custom_user_roots: &[CustomUserDataRoot],
) -> Vec<UserDataCandidateSpec> {
    custom_user_roots
        .iter()
        .filter(|root| root.path.exists())
        .map(|root| UserDataCandidateSpec {
            category: custom_user_data_category(&root.path),
            label: root
                .label
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.trim().to_string())
                .unwrap_or_else(|| default_custom_user_data_label(&root.path)),
            path: root.path.clone(),
            reason: "User-added custom migration path.".to_string(),
        })
        .collect()
}

fn custom_user_data_category(path: &Path) -> String {
    let lowered = path.display().to_string().to_ascii_lowercase();
    if lowered.contains("\\appdata\\")
        || path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with('.'))
    {
        "Custom Settings".to_string()
    } else {
        "Custom Files".to_string()
    }
}

fn default_custom_user_data_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn scan_portable_candidates_with_progress<F>(
    installed_apps: &[InstalledAppRecord],
    scan_roots: &[PathBuf],
    excluded_scan_roots: &[PathBuf],
    mut on_progress: F,
) -> anyhow::Result<Vec<PortableAppCandidate>>
where
    F: FnMut(usize, usize, String),
{
    let installed_locations: Vec<PathBuf> = installed_apps
        .iter()
        .filter_map(|app| app.install_location.clone())
        .collect();

    let mut candidates = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut evaluated_dirs = HashSet::new();
    let excluded_keys: Vec<String> = excluded_scan_roots
        .iter()
        .map(|path| path_key(path))
        .collect();
    let total_roots = scan_roots.len().max(1);

    for (index, root) in scan_roots.iter().enumerate() {
        if is_path_excluded(root, excluded_keys.as_slice()) {
            on_progress(
                index + 1,
                total_roots,
                format!(
                    "已检查 {}/{} 个扫描根，跳过排除路径 {}",
                    index + 1,
                    total_roots,
                    root.display()
                ),
            );
            continue;
        }

        if !root.exists() {
            on_progress(
                index + 1,
                total_roots,
                format!(
                    "已检查 {}/{} 个扫描根，跳过不存在路径 {}",
                    index + 1,
                    total_roots,
                    root.display()
                ),
            );
            continue;
        }

        let mut scanned_entries = 0_usize;
        let mut found_executables = 0_usize;
        let mut walker = WalkDir::new(root).min_depth(1).into_iter();
        while let Some(entry) = walker.next() {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            scanned_entries += 1;
            if is_path_excluded(path, excluded_keys.as_slice()) {
                if entry.file_type().is_dir() {
                    walker.skip_current_dir();
                }
                continue;
            }
            if entry.file_type().is_dir() && should_skip_portable_search_dir(&root, path) {
                walker.skip_current_dir();
                continue;
            }
            if is_installed_location(path, &installed_locations) || is_known_noise(path) {
                if entry.file_type().is_dir() {
                    walker.skip_current_dir();
                }
                continue;
            }

            if entry.file_type().is_file() && is_executable_file(path) {
                found_executables += 1;
                let mut accepted = false;

                if let Some(parent) = path.parent().filter(|parent| parent.file_name().is_some()) {
                    let parent_key = path_key(parent);
                    if evaluated_dirs.insert(parent_key) {
                        if let Some(candidate) =
                            evaluate_portable_directory(parent, installed_locations.as_slice())
                        {
                            let root_key = path_key(&candidate.root_path);
                            if seen_roots.insert(root_key) {
                                candidates.push(candidate);
                            }
                            accepted = true;
                        }
                    } else if seen_roots.contains(&path_key(parent)) {
                        accepted = true;
                    }
                }

                if !accepted {
                    if let Some(candidate) =
                        evaluate_portable_executable(path, installed_locations.as_slice())
                    {
                        let root_key = path_key(&candidate.root_path);
                        if seen_roots.insert(root_key) {
                            candidates.push(candidate);
                        }
                    }
                }
            }

            if scanned_entries % 400 == 0 {
                on_progress(
                    index + 1,
                    total_roots,
                    format!(
                        "正在扫描 {}/{} 个扫描根：{}，已检查 {} 个条目，发现 {} 个 exe，识别 {} 个候选",
                        index + 1,
                        total_roots,
                        root.display(),
                        scanned_entries,
                        found_executables,
                        candidates.len()
                    ),
                );
            }
        }

        if root.is_file() && is_executable_file(root) {
            found_executables += 1;
            if let Some(candidate) =
                evaluate_portable_executable(root, installed_locations.as_slice())
            {
                let root_key = path_key(&candidate.root_path);
                if seen_roots.insert(root_key) {
                    candidates.push(candidate);
                }
            }
        }

        on_progress(
            index + 1,
            total_roots,
            format!(
                "已完成 {}/{} 个扫描根：{}，共检查 {} 个条目，发现 {} 个 exe，识别 {} 个候选",
                index + 1,
                total_roots,
                root.display(),
                scanned_entries,
                found_executables,
                candidates.len()
            ),
        );
    }

    candidates.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    Ok(candidates)
}

fn is_path_excluded(path: &Path, excluded_keys: &[String]) -> bool {
    let current = path_key(path);
    excluded_keys
        .iter()
        .any(|excluded| current == *excluded || current.starts_with(&format!("{excluded}\\")))
}

fn scale_progress(start: f32, end: f32, current: usize, total: usize) -> f32 {
    if total == 0 {
        return end;
    }
    let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
    start + (end - start) * ratio
}

pub fn default_scan_roots() -> Vec<PathBuf> {
    existing_windows_drives()
}

fn is_drive_root(path: &Path) -> bool {
    let text = path.display().to_string();
    text.len() == 3 && text.ends_with(":\\")
}

fn should_skip_portable_search_dir(search_root: &Path, candidate: &Path) -> bool {
    if !is_drive_root(search_root) {
        return false;
    }

    let Ok(relative) = candidate.strip_prefix(search_root) else {
        return false;
    };
    if relative.components().count() != 1 {
        return false;
    }

    let Some(name) = candidate.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lowered = name.to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "windows"
            | "program files"
            | "program files (x86)"
            | "programdata"
            | "$recycle.bin"
            | "system volume information"
            | "recovery"
            | "msocache"
            | "perf logs"
    )
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
            | ".venv"
            | "venv"
            | "env"
            | "__pycache__"
            | "site-packages"
            | "distlib"
    )
}

#[derive(Debug, Clone, Default)]
struct PortableDirectoryEvidence {
    eligible_executables: Vec<PathBuf>,
    support_file_hits: usize,
    data_file_hits: usize,
    app_support_dir_hits: usize,
    development_marker_hits: usize,
    installer_executable_hits: usize,
    auxiliary_executable_hits: usize,
    weak_eligible_executable_hits: usize,
    curated_location: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserDataValueKind {
    Documents,
    Media,
    Archives,
    DiskImages,
    DeveloperData,
    AppData,
}

#[derive(Debug, Clone, Default)]
struct UserDataDirectoryEvidence {
    document_files: usize,
    media_files: usize,
    archive_files: usize,
    disk_image_files: usize,
    notebook_files: usize,
    app_data_files: usize,
    android_avd_marker_files: usize,
    android_avd_child_dirs: usize,
    android_sdk_component_dirs: usize,
    config_dir_hits: usize,
    executable_files: usize,
    total_files: usize,
    wallpaper_name_hint: bool,
    notebook_name_hint: bool,
}

fn evaluate_portable_directory(
    path: &Path,
    installed_locations: &[PathBuf],
) -> Option<PortableAppCandidate> {
    if is_system_install_path(path) || is_installed_location(path, installed_locations) {
        return None;
    }

    let evidence = collect_portable_directory_evidence(path);
    if evidence.eligible_executables.is_empty() {
        return None;
    }

    let main_executable = evidence
        .eligible_executables
        .iter()
        .max_by_key(|candidate| score_executable_name(path, candidate))
        .cloned()?;
    let display_name = infer_portable_display_name(path, &main_executable)?;
    if is_weak_portable_candidate_name(&display_name) {
        return None;
    }
    if !portable_directory_passes_evidence_threshold(
        path,
        &display_name,
        &main_executable,
        &evidence,
    ) {
        return None;
    }
    let confidence = portable_directory_confidence(&display_name, path, &evidence);
    let reasons = portable_directory_reasons(&evidence);

    let stats = estimate_path_stats(path);

    Some(PortableAppCandidate {
        display_name,
        root_path: path.to_path_buf(),
        main_executable,
        confidence,
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
        || is_probable_auxiliary_executable(path)
    {
        return None;
    }

    let display_name = infer_portable_display_name(path, path)?;
    let curated_location = looks_like_curated_portable_location(path);
    let sibling_support_hits = count_sibling_portable_support_files(path);
    let parent_supports_candidate = path
        .parent()
        .is_some_and(|parent| directory_name_supports_candidate(parent, &display_name));
    let parent_evidence = path
        .parent()
        .filter(|parent| parent.file_name().is_some())
        .map(collect_portable_directory_evidence);
    let parent_has_structure = sibling_support_hits > 0
        || parent_evidence.as_ref().is_some_and(|evidence| {
            evidence.data_file_hits > 0
                || evidence.app_support_dir_hits > 0
                || evidence.support_file_hits > sibling_support_hits
        });
    let parent_has_development_context = parent_evidence
        .as_ref()
        .is_some_and(|evidence| evidence.development_marker_hits >= 2);

    if !curated_location && !parent_supports_candidate && !parent_has_structure {
        return None;
    }
    if !curated_location && parent_has_development_context && sibling_support_hits == 0 {
        return None;
    }

    let confidence = if curated_location {
        PortableConfidence::High
    } else if parent_supports_candidate || parent_has_structure {
        PortableConfidence::Medium
    } else {
        PortableConfidence::Low
    };
    let stats = estimate_path_stats(path);

    let mut reasons = vec!["single executable candidate found".to_string()];
    if curated_location {
        reasons.push("stored under a common portable-app location".to_string());
    }
    if sibling_support_hits > 0 {
        reasons.push(format!(
            "{sibling_support_hits} sibling support/config file(s) found"
        ));
    }
    if parent_supports_candidate {
        reasons.push("parent folder name matches the executable".to_string());
    }

    if !curated_location && is_weak_portable_candidate_name(&display_name) {
        return None;
    }

    Some(PortableAppCandidate {
        display_name,
        root_path: path.to_path_buf(),
        main_executable: path.to_path_buf(),
        confidence,
        stats,
        reasons,
    })
}

fn collect_portable_directory_evidence(path: &Path) -> PortableDirectoryEvidence {
    let mut evidence = PortableDirectoryEvidence {
        curated_location: looks_like_curated_portable_location(path),
        ..PortableDirectoryEvidence::default()
    };

    for entry in WalkDir::new(path)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
    {
        let entry_path = entry.path();
        if entry.file_type().is_dir() {
            if entry.depth() == 1 {
                let Some(name) = entry_path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if is_portable_app_support_directory_name(name) {
                    evidence.app_support_dir_hits += 1;
                } else if is_development_marker_directory_name(name) {
                    evidence.development_marker_hits += 1;
                }
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let is_development_marker_file = entry.depth() == 1
            && entry_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(is_development_marker_file_name);
        if is_development_marker_file {
            evidence.development_marker_hits += 1;
        }

        let extension = entry_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());

        match extension.as_deref() {
            Some("exe") => {
                if is_probable_installer_executable(entry_path) {
                    evidence.installer_executable_hits += 1;
                } else if is_probable_auxiliary_executable(entry_path) {
                    evidence.auxiliary_executable_hits += 1;
                } else {
                    if entry_path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .is_some_and(is_weak_portable_candidate_name)
                    {
                        evidence.weak_eligible_executable_hits += 1;
                    }
                    evidence.eligible_executables.push(entry_path.to_path_buf());
                }
            }
            Some("dll" | "ini" | "json" | "yaml" | "toml" | "db")
                if !is_development_marker_file =>
            {
                evidence.support_file_hits += 1;
            }
            Some("dat" | "sqlite" | "xml") if !is_development_marker_file => {
                evidence.data_file_hits += 1;
            }
            _ => {}
        }
    }

    evidence
}

fn portable_directory_passes_evidence_threshold(
    path: &Path,
    display_name: &str,
    main_executable: &Path,
    evidence: &PortableDirectoryEvidence,
) -> bool {
    if evidence.eligible_executables.is_empty() {
        return false;
    }

    let directory_name_matches = directory_name_supports_candidate(path, display_name);
    let has_structural_support =
        evidence.support_file_hits + evidence.data_file_hits + evidence.app_support_dir_hits > 0;

    if !evidence.curated_location && !has_structural_support && !directory_name_matches {
        return false;
    }

    if !evidence.curated_location
        && evidence.development_marker_hits >= 2
        && evidence.support_file_hits == 0
        && evidence.data_file_hits == 0
        && evidence.app_support_dir_hits == 0
    {
        return false;
    }

    if !evidence.curated_location
        && evidence.eligible_executables.len() >= 5
        && !has_structural_support
    {
        return false;
    }

    let mut positive = 0_i32;
    if evidence.curated_location {
        positive += 4;
    }
    if directory_name_matches {
        positive += 4;
    }
    if main_executable.parent() == Some(path) {
        positive += 1;
    }
    if has_structural_support {
        positive += 1;
    }
    positive += evidence.support_file_hits.min(4) as i32;
    positive += evidence.data_file_hits.min(2) as i32;
    positive += (evidence.app_support_dir_hits.min(2) as i32) * 2;
    if evidence.eligible_executables.len() == 1 {
        positive += 1;
    }
    if evidence.weak_eligible_executable_hits < evidence.eligible_executables.len() {
        positive += 1;
    }

    let mut risk = 0_i32;
    if !has_structural_support {
        risk += 3;
    }
    if evidence.eligible_executables.len() >= 4 {
        risk += 2;
    }
    if evidence.eligible_executables.len() >= 7 {
        risk += 3;
    }
    risk += evidence.weak_eligible_executable_hits.min(3) as i32;
    if evidence.installer_executable_hits > 0 {
        risk += 1;
    }
    if evidence.auxiliary_executable_hits > evidence.eligible_executables.len() {
        risk += 2;
    }
    if evidence.development_marker_hits > 0 && !evidence.curated_location {
        risk += 3;
    }

    positive >= 4 && positive > risk
}

fn portable_directory_confidence(
    display_name: &str,
    root_path: &Path,
    evidence: &PortableDirectoryEvidence,
) -> PortableConfidence {
    let directory_name_matches = directory_name_supports_candidate(root_path, display_name);
    if evidence.curated_location
        || evidence.support_file_hits >= 3
        || evidence.app_support_dir_hits >= 2
        || (directory_name_matches && evidence.support_file_hits + evidence.data_file_hits >= 1)
    {
        PortableConfidence::High
    } else if directory_name_matches
        || evidence.support_file_hits + evidence.data_file_hits >= 1
        || evidence.app_support_dir_hits >= 1
    {
        PortableConfidence::Medium
    } else {
        PortableConfidence::Low
    }
}

fn portable_directory_reasons(evidence: &PortableDirectoryEvidence) -> Vec<String> {
    let mut reasons = vec![format!(
        "{} executable(s) found",
        evidence.eligible_executables.len()
    )];
    if evidence.support_file_hits > 0 {
        reasons.push(format!(
            "{} support/config file(s) found",
            evidence.support_file_hits
        ));
    }
    if evidence.data_file_hits > 0 {
        reasons.push(format!(
            "{} portable data file(s) found",
            evidence.data_file_hits
        ));
    }
    if evidence.app_support_dir_hits > 0 {
        reasons.push(format!(
            "{} app support folder(s) found",
            evidence.app_support_dir_hits
        ));
    }
    reasons
}

fn score_executable_name(container_root: &Path, path: &Path) -> usize {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let container_name = container_root
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();

    let mut score: usize = 0;
    if !file_name.contains("setup") && !file_name.contains("uninstall") {
        score += 5;
    }
    if !file_name.contains("helper") && !file_name.contains("update") {
        score += 3;
    }
    let weak_name = is_weak_portable_candidate_name(&stem);
    if !weak_name {
        score += 24;
    }
    if !weak_name && names_roughly_match(&stem, &container_name) {
        score += 30;
    }
    if is_generic_launcher_name(&stem) {
        score = score.saturating_sub(12);
    }
    score + file_name.len()
}

fn infer_portable_display_name(root_path: &Path, main_executable: &Path) -> Option<String> {
    let root_name = if root_path.extension().is_some() {
        root_path.file_stem().and_then(|value| value.to_str())
    } else {
        root_path.file_name().and_then(|value| value.to_str())
    };
    let parent_name = root_path
        .is_dir()
        .then(|| {
            main_executable
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
        })
        .flatten();
    let executable_name = main_executable.file_stem().and_then(|value| value.to_str());

    let mut best_name: Option<String> = None;
    let mut best_score = isize::MIN;

    for raw_name in [root_name, parent_name, executable_name]
        .into_iter()
        .flatten()
    {
        let Some(normalized) = normalize_portable_display_name(raw_name) else {
            continue;
        };

        let mut score = 0_isize;
        if Some(raw_name) == executable_name {
            score += 40;
        }
        if Some(raw_name) == root_name {
            score += 12;
        }
        if Some(raw_name) == parent_name {
            score += 8;
        }
        if names_roughly_match(&normalized, raw_name) {
            score += 4;
        }
        if !is_weak_portable_candidate_name(&normalized) {
            score += 16;
        }
        if is_generic_launcher_name(&normalized) {
            score -= 18;
        }

        if score > best_score {
            best_score = score;
            best_name = Some(normalized);
        }
    }

    best_name
}

fn normalize_portable_display_name(value: &str) -> Option<String> {
    let normalized = value.trim().trim_matches('.').to_string();
    if normalized.is_empty() || !has_meaningful_name_chars(&normalized) {
        return None;
    }
    if is_version_like_name(&normalized) || is_generic_container_name(&normalized) {
        return None;
    }
    Some(normalized)
}

fn has_meaningful_name_chars(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_alphabetic() || is_cjk_char(ch))
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2CEAF
    )
}

fn is_version_like_name(value: &str) -> bool {
    let compact: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    !compact.is_empty()
        && compact
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-' | '+' | 'v' | 'V'))
}

fn is_generic_container_name(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "app"
            | "apps"
            | "application"
            | "bin"
            | "debug"
            | "release"
            | "current"
            | "latest"
            | "tools"
            | "x64"
            | "x86"
            | "win64"
            | "win32"
            | "amd64"
            | "arm64"
    )
}

fn is_generic_launcher_name(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "app" | "launcher" | "launch" | "start" | "run" | "helper" | "bootstrap"
    )
}

fn is_weak_portable_candidate_name(value: &str) -> bool {
    is_version_like_name(value)
        || is_generic_container_name(value)
        || is_generic_launcher_name(value)
}

fn count_sibling_portable_support_files(path: &Path) -> usize {
    let Some(parent) = path.parent() else {
        return 0;
    };
    let Some(current_name) = path.file_name().and_then(|value| value.to_str()) else {
        return 0;
    };

    fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|value| value.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                !name.eq_ignore_ascii_case(current_name) && !is_development_marker_file_name(name)
            })
        })
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "dll" | "ini" | "json" | "yaml" | "toml" | "db" | "dat" | "sqlite" | "xml"
                    )
                })
        })
        .count()
}

fn is_portable_app_support_directory_name(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "config"
            | "configs"
            | "data"
            | "plugins"
            | "plugin"
            | "resources"
            | "resource"
            | "themes"
            | "theme"
            | "locale"
            | "locales"
            | "lang"
            | "language"
            | "extensions"
            | "extension"
            | "profiles"
            | "profile"
    )
}

fn is_development_marker_directory_name(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        ".git"
            | ".github"
            | ".idea"
            | ".vscode"
            | "src"
            | "test"
            | "tests"
            | "example"
            | "examples"
            | ".venv"
            | "venv"
            | "env"
            | "node_modules"
            | "site-packages"
            | "distlib"
            | "__pycache__"
    )
}

fn is_development_marker_file_name(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    matches!(
        lowered.as_str(),
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "pyproject.toml"
            | "poetry.lock"
            | "requirements.txt"
            | "setup.py"
            | "setup.cfg"
            | "go.mod"
            | ".gitignore"
    )
}

fn directory_name_supports_candidate(path: &Path, display_name: &str) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| !is_generic_container_name(name) && names_roughly_match(name, display_name))
        .unwrap_or(false)
}

fn names_roughly_match(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .flat_map(|ch| ch.to_lowercase())
            .collect::<String>()
    };
    let left = normalize(left);
    let right = normalize(right);
    !left.is_empty() && left == right
}

fn is_executable_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
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

fn is_probable_auxiliary_executable(path: &Path) -> bool {
    let file_name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    if matches!(file_name.as_str(), "t32" | "t64" | "w32" | "w64") {
        return true;
    }

    if is_python_toolchain_wrapper_name(&file_name) && has_python_toolchain_context(path) {
        return true;
    }

    [
        "launcher",
        "launch",
        "helper",
        "service",
        "broker",
        "daemon",
        "monitor",
        "updater",
        "updatehelper",
        "crashpad_handler",
        "crashreporter",
        "crashreport",
        "unitycrashhandler",
        "subprocess",
        "renderer",
        "cefsharp.browser",
        "webview2",
        "elevation_service",
        "notification_helper",
        "proxy",
        "host",
        "bootstrap",
    ]
    .iter()
    .any(|pattern| file_name == *pattern || file_name.contains(pattern))
}

fn has_python_toolchain_context(path: &Path) -> bool {
    path.ancestors()
        .filter_map(|ancestor| ancestor.file_name())
        .filter_map(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .any(|segment| {
            matches!(
                segment.as_str(),
                ".venv"
                    | "venv"
                    | "env"
                    | "scripts"
                    | "site-packages"
                    | "distlib"
                    | "__pypackages__"
            )
        })
}

fn is_python_toolchain_wrapper_name(file_name: &str) -> bool {
    if matches!(
        file_name,
        "python" | "pythonw" | "py" | "pyw" | "pip" | "wheel" | "easy_install" | "idle" | "f2py"
    ) {
        return true;
    }

    file_name.starts_with("pip")
        || file_name.starts_with("python3")
        || file_name.starts_with("pythonw3")
        || file_name.starts_with("easy_install-")
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
        classify_scan_root_user_directory_candidate, classify_scan_root_user_file_candidate,
        custom_user_data_category, default_custom_user_data_label, default_user_data_candidates,
        discover_scan_root_user_data_candidates, evaluate_portable_directory,
        evaluate_portable_executable, filter_user_data_roots_against_portable_candidates,
        infer_portable_display_name, is_known_noise, is_system_install_path,
        normalize_portable_display_name,
    };
    use crate::models::{PathStats, PortableAppCandidate, PortableConfidence, UserDataRoot};
    use std::fs;
    use std::path::{Path, PathBuf};
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
        assert!(matches!(
            candidate.confidence,
            super::PortableConfidence::High
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_single_executable_candidate_with_sibling_support_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-single-exe-{unique}"));
        fs::create_dir_all(&root).expect("create single-exe test root");
        let exe = root.join("Tool.exe");
        fs::write(&exe, b"exe").expect("write exe");
        fs::write(root.join("tool.ini"), b"ini").expect("write support file");

        let candidate =
            evaluate_portable_executable(&exe, &[]).expect("single executable candidate expected");

        assert_eq!(candidate.display_name, "Tool");
        assert_eq!(candidate.root_path, exe);
        assert_eq!(candidate.main_executable, candidate.root_path);
        assert_eq!(candidate.stats.file_count, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn portable_directory_prefers_meaningful_executable_name_over_numeric_folder_name() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-portable-name-{unique}"));
        let app_dir = root.join("12345");
        fs::create_dir_all(&app_dir).expect("create numeric dir");
        fs::write(app_dir.join("12345.exe"), b"exe").expect("write numeric exe");
        fs::write(app_dir.join("CoolApp.exe"), b"exe").expect("write app exe");
        fs::write(app_dir.join("config.ini"), b"ini").expect("write config");

        let candidate =
            evaluate_portable_directory(&app_dir, &[]).expect("portable candidate expected");

        assert_eq!(candidate.display_name, "CoolApp");
        assert_eq!(candidate.main_executable, app_dir.join("CoolApp.exe"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_numeric_single_executable_name() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-single-numeric-{unique}"));
        fs::create_dir_all(&root).expect("create single-exe test root");
        let exe = root.join("12345.exe");
        fs::write(&exe, b"exe").expect("write exe");

        assert!(evaluate_portable_executable(&exe, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn display_name_helpers_reject_versions_and_keep_real_names() {
        assert_eq!(
            normalize_portable_display_name("CoolApp"),
            Some("CoolApp".to_string())
        );
        assert!(normalize_portable_display_name("1.2.3").is_none());
        assert!(normalize_portable_display_name("12345").is_none());
        assert!(normalize_portable_display_name("x64").is_none());
    }

    #[test]
    fn display_name_inference_uses_executable_when_container_name_is_generic() {
        let root = Path::new("D:\\Tools\\x64");
        let main_executable = Path::new("D:\\Tools\\x64\\ScreenToGif.exe");

        let inferred = infer_portable_display_name(root, main_executable);

        assert_eq!(inferred.as_deref(), Some("ScreenToGif"));
    }

    #[test]
    fn rejects_auxiliary_single_executable_names() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-single-launcher-{unique}"));
        fs::create_dir_all(&root).expect("create launcher test root");
        let exe = root.join("Launcher.exe");
        fs::write(&exe, b"exe").expect("write launcher exe");

        assert!(evaluate_portable_executable(&exe, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_distlib_launcher_executable_names() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-distlib-launcher-{unique}"));
        fs::create_dir_all(&root).expect("create distlib launcher test root");

        for name in ["t32.exe", "t64.exe", "w32.exe", "w64.exe"] {
            let exe = root.join(name);
            fs::write(&exe, b"exe").expect("write distlib launcher exe");
            assert!(evaluate_portable_executable(&exe, &[]).is_none());
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_python_toolchain_wrappers_inside_venv_context() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-venv-wrapper-{unique}"));
        let scripts = root.join(".venv\\Scripts");
        fs::create_dir_all(&scripts).expect("create scripts dir");

        for name in [
            "python.exe",
            "pythonw.exe",
            "pip.exe",
            "pip3.exe",
            "wheel.exe",
            "easy_install.exe",
        ] {
            let exe = scripts.join(name);
            fs::write(&exe, b"exe").expect("write toolchain wrapper exe");
            assert!(evaluate_portable_executable(&exe, &[]).is_none());
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_single_exe_directory_without_support_or_matching_folder() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-weak-dir-{unique}"));
        let app_dir = root.join("12345");
        fs::create_dir_all(&app_dir).expect("create weak dir");
        fs::write(app_dir.join("CoolApp.exe"), b"exe").expect("write app exe");

        assert!(evaluate_portable_directory(&app_dir, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_directory_that_looks_like_source_project_not_portable_app() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-source-project-{unique}"));
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::create_dir_all(root.join(".venv")).expect("create venv dir");
        fs::write(root.join("MediaCrawlerPro.exe"), b"exe").expect("write app exe");
        fs::write(root.join("pyproject.toml"), b"toml").expect("write pyproject");

        assert!(evaluate_portable_directory(&root, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_single_executable_inside_source_project_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-source-single-{unique}"));
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::create_dir_all(root.join(".venv")).expect("create venv dir");
        let exe = root.join("MediaCrawlerPro.exe");
        fs::write(&exe, b"exe").expect("write app exe");
        fs::write(root.join("pyproject.toml"), b"toml").expect("write pyproject");

        assert!(evaluate_portable_executable(&exe, &[]).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_single_exe_directory_when_folder_matches_program_name() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-strong-dir-{unique}"));
        let app_dir = root.join("CoolApp");
        fs::create_dir_all(&app_dir).expect("create app dir");
        fs::write(app_dir.join("CoolApp.exe"), b"exe").expect("write app exe");

        let candidate =
            evaluate_portable_directory(&app_dir, &[]).expect("matching folder should pass");

        assert_eq!(candidate.display_name, "CoolApp");

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
        assert!(is_known_noise(Path::new(
            "C:\\MediaCrawlerPro\\MediaCrawlerPro-Python\\.venv"
        )));
        assert!(is_known_noise(Path::new(
            "C:\\MediaCrawlerPro\\MediaCrawlerPro-Python\\.venv\\Lib\\site-packages"
        )));
        assert!(is_known_noise(Path::new(
            "C:\\MediaCrawlerPro\\MediaCrawlerPro-Python\\.venv\\Lib\\site-packages\\distlib"
        )));
        assert!(!is_known_noise(Path::new("D:\\foo\\binary\\app.exe")));
    }

    #[test]
    fn user_data_candidates_cover_more_high_value_settings() {
        let profile = Path::new("C:\\Users\\Sunny");
        let roaming = Path::new("C:\\Users\\Sunny\\AppData\\Roaming");
        let local = Path::new("C:\\Users\\Sunny\\AppData\\Local");
        let candidates = default_user_data_candidates(profile, roaming, local);
        let labels: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.label.as_str())
            .collect();

        assert!(labels.contains(&"Saved Games"));
        assert!(labels.contains(&"PowerShell Profile"));
        assert!(labels.contains(&"Firefox Profiles"));
        assert!(labels.contains(&"Notepad++"));
        assert!(labels.contains(&"OBS Studio"));
    }

    #[test]
    fn custom_user_data_helpers_detect_settings_and_default_labels() {
        assert_eq!(
            custom_user_data_category(Path::new(
                "C:\\Users\\Sunny\\AppData\\Roaming\\Foo\\config.json"
            )),
            "Custom Settings"
        );
        assert_eq!(
            custom_user_data_category(Path::new("D:\\Archive\\Research")),
            "Custom Files"
        );
        assert_eq!(
            default_custom_user_data_label(Path::new("D:\\Archive\\Research")),
            "Research"
        );
        assert_eq!(
            default_custom_user_data_label(Path::new("C:\\Users\\Sunny\\.tool-rc")),
            ".tool-rc"
        );
    }

    #[test]
    fn classifies_root_level_disk_image_file_candidate() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-user-file-{unique}"));
        fs::create_dir_all(&root).expect("create root dir");
        let iso = root.join("zh-cn_windows_11_business.iso");
        fs::write(&iso, b"iso").expect("write iso");

        let candidate =
            classify_scan_root_user_file_candidate(&iso).expect("iso file should be classified");
        assert_eq!(candidate.category, "Disk Images");
        assert_eq!(candidate.label, "zh-cn_windows_11_business.iso");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_directory_candidates_from_content_evidence() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-user-dir-{unique}"));
        let wallpapers = root.join("Wallpapers");
        let notebooks = root.join("Research");
        let avd = root.join("AndroidDevices");
        let app_data = root.join("ChatData");
        fs::create_dir_all(&wallpapers).expect("create wallpapers dir");
        fs::create_dir_all(&notebooks).expect("create notebooks dir");
        fs::create_dir_all(avd.join("Pixel_9.avd")).expect("create avd child dir");
        fs::create_dir_all(app_data.join("config")).expect("create app config dir");
        fs::create_dir_all(app_data.join("backup")).expect("create app backup dir");
        fs::write(wallpapers.join("1.jpg"), b"jpg").expect("write image 1");
        fs::write(notebooks.join("analysis.ipynb"), b"{}").expect("write notebook");
        fs::write(avd.join("config.ini"), b"[avd]").expect("write avd config");
        fs::write(avd.join("userdata-qemu.img"), b"img").expect("write avd image");
        fs::write(app_data.join("config\\global_config"), b"cfg").expect("write config file");
        fs::write(app_data.join("config\\key_info.db"), b"db").expect("write db file");
        fs::write(app_data.join("backup\\session.dat"), b"dat").expect("write dat file");
        fs::write(app_data.join("backup\\meta.json"), b"json").expect("write json file");

        let wallpaper_candidate = classify_scan_root_user_directory_candidate(&wallpapers)
            .expect("wallpaper dir should be classified");
        let notebook_candidate = classify_scan_root_user_directory_candidate(&notebooks)
            .expect("notebook dir should be classified");
        let avd_candidate = classify_scan_root_user_directory_candidate(&avd)
            .expect("avd dir should be classified");
        let app_data_candidate = classify_scan_root_user_directory_candidate(&app_data)
            .expect("app data dir should be classified");

        assert_eq!(wallpaper_candidate.category, "Personal Media");
        assert_eq!(notebook_candidate.category, "Developer Data");
        assert_eq!(avd_candidate.category, "Developer Data");
        assert_eq!(app_data_candidate.category, "App Data");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovers_scan_root_user_data_candidates_from_extra_drives() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-user-scan-{unique}"));
        let wallpapers = root.join("Wallpapers");
        let avd = root.join("android");
        let notebooks = root.join("work\\research");
        let chat_data = root.join("chat");
        let excluded = root.join("excluded\\Wallpapers");
        let iso = root.join("zh-cn_windows_11_business.iso");

        fs::create_dir_all(&wallpapers).expect("create wallpapers dir");
        fs::create_dir_all(&avd).expect("create avd dir");
        fs::create_dir_all(&notebooks).expect("create notebooks dir");
        fs::create_dir_all(chat_data.join("config")).expect("create chat config dir");
        fs::create_dir_all(chat_data.join("nt_db")).expect("create chat db dir");
        fs::create_dir_all(&excluded).expect("create excluded dir");
        fs::create_dir_all(avd.join("Pixel_9.avd")).expect("create avd child dir");
        fs::write(wallpapers.join("1.jpg"), b"jpg").expect("write image 1");
        fs::write(notebooks.join("analysis.ipynb"), b"{}").expect("write notebook file");
        fs::write(avd.join("config.ini"), b"[avd]").expect("write avd config");
        fs::write(avd.join("userdata-qemu.img"), b"img").expect("write avd image");
        fs::write(chat_data.join("config\\global_config"), b"cfg").expect("write config file");
        fs::write(chat_data.join("config\\client_config"), b"cfg").expect("write config file 2");
        fs::write(chat_data.join("nt_db\\group_info.db"), b"db").expect("write db file");
        fs::write(chat_data.join("nt_db\\group_info.db-wal"), b"wal").expect("write wal file");
        fs::write(&iso, b"iso").expect("write iso");

        let candidates = discover_scan_root_user_data_candidates(
            std::slice::from_ref(&root),
            std::slice::from_ref(&root.join("excluded")),
        );
        let labels: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.label.as_str())
            .collect();
        let categories: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.category.as_ref())
            .collect();
        let paths: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.path.display().to_string())
            .collect();

        assert!(labels.contains(&"Wallpapers"));
        assert!(labels.contains(&"android"));
        assert!(labels.contains(&"research") || labels.contains(&"work"));
        assert!(labels.contains(&"chat"));
        assert!(labels.contains(&"zh-cn_windows_11_business.iso"));
        assert!(categories.contains(&"Disk Images"));
        assert!(categories.contains(&"App Data"));
        assert!(!paths.iter().any(|path| path.contains("excluded")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn collapses_android_sdk_like_tree_into_single_parent_candidate() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-android-sdk-{unique}"));
        let sdk = root.join("android-sdk");
        fs::create_dir_all(sdk.join("platforms\\android-35")).expect("create platforms");
        fs::create_dir_all(sdk.join("skins\\pixel")).expect("create skins");
        fs::create_dir_all(sdk.join("sources\\android-35")).expect("create sources");
        fs::create_dir_all(sdk.join("system-images\\android-35")).expect("create system images");
        fs::write(sdk.join("platforms\\android-35\\android.jar"), b"jar").expect("write jar");
        fs::write(sdk.join("sources\\android-35\\source.properties"), b"props")
            .expect("write source props");

        let candidates = discover_scan_root_user_data_candidates(std::slice::from_ref(&root), &[]);
        let labels: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.label.as_str())
            .collect();
        let paths: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.path.display().to_string())
            .collect();

        assert!(labels.contains(&"android-sdk"));
        assert!(!paths.iter().any(|path| path.ends_with("\\platforms")));
        assert!(!paths.iter().any(|path| path.ends_with("\\skins")));
        assert!(!paths.iter().any(|path| path.ends_with("\\sources")));
        assert!(!paths.iter().any(|path| path.ends_with("\\system-images")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn removes_personal_candidates_nested_under_portable_root() {
        let roots = vec![
            UserDataRoot {
                category: "Personal Media".into(),
                label: "images".into(),
                path: PathBuf::from("C:\\die\\images"),
                reason: "nested".into(),
                stats: PathStats {
                    file_count: 10,
                    total_bytes: 1024,
                },
            },
            UserDataRoot {
                category: "App Data".into(),
                label: "peid".into(),
                path: PathBuf::from("C:\\die\\peid"),
                reason: "nested".into(),
                stats: PathStats {
                    file_count: 2,
                    total_bytes: 512,
                },
            },
            UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: PathBuf::from("D:\\Documents"),
                reason: "outside".into(),
                stats: PathStats {
                    file_count: 3,
                    total_bytes: 256,
                },
            },
        ];
        let portable_candidates = vec![PortableAppCandidate {
            display_name: "die".to_string(),
            root_path: PathBuf::from("C:\\die"),
            main_executable: PathBuf::from("C:\\die\\die.exe"),
            confidence: PortableConfidence::High,
            stats: PathStats {
                file_count: 100,
                total_bytes: 4096,
            },
            reasons: vec![],
        }];

        let filtered =
            filter_user_data_roots_against_portable_candidates(roots, &portable_candidates);
        let labels: Vec<&str> = filtered.iter().map(|root| root.label.as_ref()).collect();

        assert_eq!(filtered.len(), 1);
        assert!(labels.contains(&"Documents"));
        assert!(!labels.contains(&"images"));
        assert!(!labels.contains(&"peid"));
    }
}
