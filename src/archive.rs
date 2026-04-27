use crate::plan::{path_key, should_exclude_path};
use anyhow::{Context, bail};
use crc32fast::Hasher;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const HEADER_MAGIC: &[u8; 4] = b"WRH1";
const FOOTER_MAGIC: &[u8; 4] = b"WRHF";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct BackupResult {
    pub archive_path: PathBuf,
    pub file_count: usize,
    pub original_bytes: u64,
    pub stored_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct RestoreResult {
    pub archive_path: PathBuf,
    pub destination_root: PathBuf,
    pub restored_files: usize,
    pub restored_bytes: u64,
    pub skipped_existing_files: usize,
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub archive_path: PathBuf,
    pub verified_files: usize,
    pub verified_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct BackupOutputPreflight {
    pub output_dir: PathBuf,
    pub exists: bool,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RestorePreflight {
    pub selected_files: usize,
    pub new_files: usize,
    pub conflicting_files: usize,
    pub new_examples: Vec<String>,
    pub conflict_examples: Vec<String>,
    pub destination_exists: bool,
    pub destination_is_directory: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreProgress {
    pub processed_files: usize,
    pub total_files: usize,
    pub current_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSelection {
    pub restore_user_data: bool,
    pub restore_portable_apps: bool,
    pub restore_installed_app_dirs: bool,
    pub selected_roots: HashSet<String>,
    pub skip_existing_files: bool,
}

impl Default for RestoreSelection {
    fn default() -> Self {
        Self {
            restore_user_data: true,
            restore_portable_apps: true,
            restore_installed_app_dirs: true,
            selected_roots: HashSet::new(),
            skip_existing_files: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub format_version: u32,
    pub created_at_unix: u64,
    pub app_name: String,
    pub app_version: String,
    pub installed_apps: Vec<ManifestInstalledApp>,
    pub selected_user_roots: Vec<ManifestRoot>,
    pub selected_portable_apps: Vec<ManifestPortableApp>,
    pub files: Vec<ArchivedFileEntry>,
    pub original_bytes: u64,
    pub stored_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInstalledApp {
    pub display_name: String,
    pub source: String,
    pub install_location: Option<String>,
    #[serde(default)]
    pub backup_root: Option<String>,
    #[serde(default)]
    pub files_included: bool,
    pub uninstall_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRoot {
    pub category: String,
    pub label: String,
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPortableApp {
    pub display_name: String,
    pub root_path: String,
    pub main_executable: String,
    pub confidence: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedFileEntry {
    pub source_path: String,
    pub archive_path: String,
    pub entry_kind: String,
    pub offset: u64,
    pub stored_size: u64,
    pub original_size: u64,
    pub crc32: u32,
}

#[derive(Debug)]
struct PendingFile {
    source_path: PathBuf,
    archive_path: String,
    entry_kind: &'static str,
}

#[derive(Debug)]
struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Debug)]
struct RangeReader {
    file: File,
    remaining: u64,
}

impl RangeReader {
    fn new(mut file: File, offset: u64, remaining: u64) -> anyhow::Result<Self> {
        file.seek(SeekFrom::Start(offset))?;
        Ok(Self { file, remaining })
    }
}

impl Read for RangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }

        let limit = self.remaining.min(buf.len() as u64) as usize;
        let read = self.file.read(&mut buf[..limit])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

pub fn create_backup_archive(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
) -> anyhow::Result<BackupResult> {
    let output_dir = default_output_dir()?;
    create_backup_archive_in_dir(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
        &output_dir,
    )
}

pub fn create_backup_archive_in_dir(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
    output_dir: &Path,
) -> anyhow::Result<BackupResult> {
    create_backup_archive_at(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
        output_dir,
    )
}

pub fn preview_backup_output(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
    output_dir: &Path,
) -> anyhow::Result<BackupOutputPreflight> {
    let metadata = fs::metadata(output_dir).ok();
    if metadata.as_ref().is_some_and(|metadata| !metadata.is_dir()) {
        bail!(
            "backup output path is an existing file: {}",
            output_dir.display()
        );
    }

    if let Some(blocker) = find_existing_file_in_ancestors(output_dir) {
        bail!(
            "backup output path is blocked by existing file: {}",
            blocker.display()
        );
    }

    for source_dir in collect_selected_backup_source_dirs(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
    ) {
        if output_dir == source_dir || output_dir.starts_with(&source_dir) {
            bail!(
                "backup output path overlaps selected source directory: {}",
                source_dir.display()
            );
        }
    }

    Ok(BackupOutputPreflight {
        output_dir: output_dir.to_path_buf(),
        exists: metadata.is_some(),
        is_directory: metadata.as_ref().is_some_and(|metadata| metadata.is_dir()),
    })
}

pub fn list_recent_archives_from_dirs(
    dirs: &[PathBuf],
    limit: usize,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut archives_by_path = HashMap::new();

    for dir in dirs {
        if !dir.exists() || !dir.is_dir() {
            continue;
        }

        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };

            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("wrh") {
                continue;
            }

            let modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
                Ok(value) => value,
                Err(_) => continue,
            };

            archives_by_path
                .entry(path)
                .and_modify(|existing: &mut SystemTime| {
                    if modified > *existing {
                        *existing = modified;
                    }
                })
                .or_insert(modified);
        }
    }

    let mut archives: Vec<(PathBuf, SystemTime)> = archives_by_path.into_iter().collect();
    archives.sort_by(|left, right| right.1.cmp(&left.1));
    let mut paths: Vec<PathBuf> = archives.into_iter().map(|(path, _)| path).collect();
    if limit > 0 && paths.len() > limit {
        paths.truncate(limit);
    }
    Ok(paths)
}

pub fn default_output_dir() -> anyhow::Result<PathBuf> {
    if let Some(profile) = env::var_os("USERPROFILE") {
        let desktop = PathBuf::from(profile)
            .join("Desktop")
            .join("WinRehome Backups");
        return Ok(desktop);
    }

    Ok(env::current_dir()?.join("WinRehome Backups"))
}

pub fn default_restore_dir(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let restore_root = if let Some(profile) = env::var_os("USERPROFILE") {
        PathBuf::from(profile)
            .join("Desktop")
            .join("WinRehome Restores")
    } else {
        env::current_dir()?.join("WinRehome Restores")
    };

    let archive_name = archive_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("restored-archive");
    Ok(restore_root.join(sanitize_segment(archive_name)))
}

#[cfg(test)]
pub fn restore_archive(
    archive_path: &Path,
    destination_root: &Path,
) -> anyhow::Result<RestoreResult> {
    let manifest = read_archive_manifest(archive_path)?;
    restore_archive_with_manifest_and_progress(
        archive_path,
        destination_root,
        &manifest,
        RestoreSelection {
            selected_roots: collect_manifest_restore_roots(&manifest),
            ..RestoreSelection::default()
        },
        |_| {},
    )
}

#[cfg(test)]
pub fn restore_archive_with_selection(
    archive_path: &Path,
    destination_root: &Path,
    selection: RestoreSelection,
) -> anyhow::Result<RestoreResult> {
    restore_archive_with_selection_and_progress(archive_path, destination_root, selection, |_| {})
}

pub fn restore_archive_with_selection_and_progress<F>(
    archive_path: &Path,
    destination_root: &Path,
    selection: RestoreSelection,
    on_progress: F,
) -> anyhow::Result<RestoreResult>
where
    F: FnMut(RestoreProgress),
{
    let manifest = read_archive_manifest(archive_path)?;
    restore_archive_with_manifest_and_progress(
        archive_path,
        destination_root,
        &manifest,
        selection,
        on_progress,
    )
}

pub fn preview_restore_with_manifest(
    destination_root: &Path,
    manifest: &ArchiveManifest,
    selection: &RestoreSelection,
) -> anyhow::Result<RestorePreflight> {
    let selected_files = collect_selected_restore_entries(manifest, selection);

    if selected_files.is_empty() {
        bail!("Archive does not contain any files to restore.");
    }

    let destination_metadata = fs::metadata(destination_root).ok();
    if destination_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_dir())
    {
        bail!(
            "restore destination is an existing file: {}",
            destination_root.display()
        );
    }

    validate_restore_targets(destination_root, &selected_files)?;

    let mut preview = RestorePreflight {
        selected_files: selected_files.len(),
        destination_exists: destination_metadata.is_some(),
        destination_is_directory: destination_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_dir()),
        ..RestorePreflight::default()
    };

    for entry in selected_files {
        let relative_restore_path = validate_restore_relative_path(&entry.archive_path)?;
        let output_path = destination_root.join(relative_restore_path);
        if output_path.exists() {
            preview.conflicting_files += 1;
            if preview.conflict_examples.len() < 3 {
                preview
                    .conflict_examples
                    .push(output_path.display().to_string());
            }
        } else {
            preview.new_files += 1;
            if preview.new_examples.len() < 3 {
                preview.new_examples.push(output_path.display().to_string());
            }
        }
    }

    Ok(preview)
}

fn restore_archive_with_manifest_and_progress<F>(
    archive_path: &Path,
    destination_root: &Path,
    manifest: &ArchiveManifest,
    selection: RestoreSelection,
    mut on_progress: F,
) -> anyhow::Result<RestoreResult>
where
    F: FnMut(RestoreProgress),
{
    let selected_files = collect_selected_restore_entries(manifest, &selection);

    if selected_files.is_empty() {
        bail!("Archive does not contain any files to restore.");
    }

    validate_restore_targets(destination_root, &selected_files)?;

    fs::create_dir_all(destination_root).with_context(|| {
        format!(
            "failed to create restore destination {}",
            destination_root.display()
        )
    })?;

    let archive = File::open(archive_path)
        .with_context(|| format!("failed to open archive {}", archive_path.display()))?;
    let mut restored_files = 0_usize;
    let mut restored_bytes = 0_u64;
    let mut skipped_existing_files = 0_usize;
    let total_files = selected_files.len();

    on_progress(RestoreProgress {
        processed_files: 0,
        total_files,
        current_path: "准备恢复文件...".to_string(),
    });

    for (index, entry) in selected_files.into_iter().enumerate() {
        let relative_restore_path = validate_restore_relative_path(&entry.archive_path)?;
        let output_path = destination_root.join(&relative_restore_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create restore directory {}", parent.display())
            })?;
        }
        if output_path.exists() {
            if selection.skip_existing_files {
                skipped_existing_files += 1;
                on_progress(RestoreProgress {
                    processed_files: index + 1,
                    total_files,
                    current_path: output_path.display().to_string(),
                });
                continue;
            }
            bail!("restore target already exists: {}", output_path.display());
        }

        let mut range_reader =
            RangeReader::new(archive.try_clone()?, entry.offset, entry.stored_size)?;
        let mut decoder = DeflateDecoder::new(&mut range_reader);
        let mut output = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        let mut hasher = Hasher::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut restored_len = 0_u64;

        loop {
            let read = decoder.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            restored_len += read as u64;
        }

        let actual_crc = hasher.finalize();
        if restored_len != entry.original_size {
            bail!(
                "restore size mismatch for {}: expected {}, got {}",
                entry.archive_path,
                entry.original_size,
                restored_len
            );
        }
        if actual_crc != entry.crc32 {
            bail!(
                "restore checksum mismatch for {}: expected {}, got {}",
                entry.archive_path,
                entry.crc32,
                actual_crc
            );
        }

        restored_files += 1;
        restored_bytes += restored_len;
        on_progress(RestoreProgress {
            processed_files: index + 1,
            total_files,
            current_path: output_path.display().to_string(),
        });
    }

    Ok(RestoreResult {
        archive_path: archive_path.to_path_buf(),
        destination_root: destination_root.to_path_buf(),
        restored_files,
        restored_bytes,
        skipped_existing_files,
    })
}

fn validate_restore_relative_path(archive_path: &str) -> anyhow::Result<PathBuf> {
    let mut relative = PathBuf::new();

    for component in Path::new(archive_path).components() {
        match component {
            Component::Normal(segment) => relative.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("archive entry path escapes restore root: {}", archive_path);
            }
        }
    }

    if relative.as_os_str().is_empty() {
        bail!("archive entry path is empty: {}", archive_path);
    }

    Ok(relative)
}

fn collect_selected_restore_entries<'a>(
    manifest: &'a ArchiveManifest,
    selection: &RestoreSelection,
) -> Vec<&'a ArchivedFileEntry> {
    manifest
        .files
        .iter()
        .filter(|entry| {
            let category_allowed = match entry.entry_kind.as_str() {
                "user_data" => selection.restore_user_data,
                "portable_app" => selection.restore_portable_apps,
                "installed_app" => selection.restore_installed_app_dirs,
                _ => false,
            };

            if !category_allowed || selection.selected_roots.is_empty() {
                return false;
            }

            selection.selected_roots.iter().any(|root| {
                entry.archive_path == *root || entry.archive_path.starts_with(&format!("{root}/"))
            })
        })
        .collect()
}

fn validate_restore_targets(
    destination_root: &Path,
    selected_files: &[&ArchivedFileEntry],
) -> anyhow::Result<()> {
    if let Some(blocker) = find_existing_file_in_ancestors(destination_root) {
        bail!(
            "restore destination is blocked by existing file: {}",
            blocker.display()
        );
    }

    let mut seen_targets = HashSet::new();
    for entry in selected_files {
        let relative_restore_path = validate_restore_relative_path(&entry.archive_path)?;
        let output_path = destination_root.join(&relative_restore_path);
        let output_key = path_key(&output_path);
        if !seen_targets.insert(output_key) {
            bail!(
                "restore target path is duplicated in archive: {}",
                output_path.display()
            );
        }

        if let Some(blocker) = find_existing_file_in_ancestors(&output_path) {
            bail!(
                "restore target is blocked by existing file: {}",
                blocker.display()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
fn collect_manifest_restore_roots(manifest: &ArchiveManifest) -> HashSet<String> {
    let mut roots = HashSet::new();
    for root in &manifest.selected_user_roots {
        roots.insert(format!(
            "user/{}/{}",
            sanitize_segment(&root.category),
            sanitize_segment(&root.label)
        ));
    }
    for app in &manifest.selected_portable_apps {
        roots.insert(format!("portable/{}", sanitize_segment(&app.display_name)));
    }
    for app in &manifest.installed_apps {
        if let Some(root) = &app.backup_root {
            roots.insert(root.clone());
        }
    }
    roots
}

pub fn verify_archive(archive_path: &Path) -> anyhow::Result<VerificationResult> {
    let manifest = read_archive_manifest(archive_path)?;
    if manifest.files.is_empty() {
        bail!("Archive does not contain any files to verify.");
    }

    let archive = File::open(archive_path)
        .with_context(|| format!("failed to open archive {}", archive_path.display()))?;
    let mut verified_files = 0_usize;
    let mut verified_bytes = 0_u64;

    for entry in &manifest.files {
        let mut range_reader =
            RangeReader::new(archive.try_clone()?, entry.offset, entry.stored_size)?;
        let mut decoder = DeflateDecoder::new(&mut range_reader);
        let mut hasher = Hasher::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut verified_len = 0_u64;

        loop {
            let read = decoder.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            verified_len += read as u64;
        }

        let actual_crc = hasher.finalize();
        if verified_len != entry.original_size {
            bail!(
                "verify size mismatch for {}: expected {}, got {}",
                entry.archive_path,
                entry.original_size,
                verified_len
            );
        }
        if actual_crc != entry.crc32 {
            bail!(
                "verify checksum mismatch for {}: expected {}, got {}",
                entry.archive_path,
                entry.crc32,
                actual_crc
            );
        }

        verified_files += 1;
        verified_bytes += verified_len;
    }

    Ok(VerificationResult {
        archive_path: archive_path.to_path_buf(),
        verified_files,
        verified_bytes,
    })
}

fn create_backup_archive_at(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
    output_dir: &Path,
) -> anyhow::Result<BackupResult> {
    let pending_files = collect_pending_files(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
    )?;
    if pending_files.is_empty() {
        bail!("No files are selected for backup.");
    }

    preview_backup_output(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
        output_dir,
    )?;

    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create backup output directory {}",
            output_dir.display()
        )
    })?;

    let created_at_unix = now_unix();
    let file_name = format!("WinRehome-backup-{created_at_unix}.wrh");
    let archive_path = output_dir.join(&file_name);
    let partial_path = archive_path.with_extension("wrh.partial");

    let mut archive = File::create(&partial_path)
        .with_context(|| format!("failed to create {}", partial_path.display()))?;
    archive.write_all(HEADER_MAGIC)?;

    let mut manifest_files = Vec::with_capacity(pending_files.len());
    let mut original_bytes = 0_u64;
    let mut stored_bytes = 0_u64;

    for pending in pending_files {
        let entry = write_file_entry(&mut archive, pending)?;
        original_bytes += entry.original_size;
        stored_bytes += entry.stored_size;
        manifest_files.push(entry);
    }

    let manifest = ArchiveManifest {
        format_version: FORMAT_VERSION,
        created_at_unix,
        app_name: "WinRehome".to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        installed_apps: preview
            .installed_apps
            .iter()
            .map(|app| ManifestInstalledApp {
                backup_root: selected_installed_app_dirs
                    .contains(&app.selection_key())
                    .then(|| installed_app_backup_root(app)),
                display_name: app.display_name.clone(),
                files_included: selected_installed_app_dirs.contains(&app.selection_key())
                    && app.can_backup_files(),
                source: app.source.to_string(),
                install_location: app
                    .install_location
                    .as_ref()
                    .map(|path| path.display().to_string()),
                uninstall_key: app.uninstall_key.clone(),
            })
            .collect(),
        selected_user_roots: preview
            .user_data_roots
            .iter()
            .filter(|root| selected_user_roots.contains(&path_key(&root.path)))
            .map(|root| ManifestRoot {
                category: root.category.to_string(),
                label: root.label.to_string(),
                path: root.path.display().to_string(),
                reason: root.reason.to_string(),
            })
            .collect(),
        selected_portable_apps: preview
            .portable_candidates
            .iter()
            .filter(|candidate| selected_portable_apps.contains(&path_key(&candidate.root_path)))
            .map(|candidate| ManifestPortableApp {
                display_name: candidate.display_name.clone(),
                root_path: candidate.root_path.display().to_string(),
                main_executable: candidate.main_executable.display().to_string(),
                confidence: candidate.confidence_label().to_string(),
                reasons: candidate.reasons.clone(),
            })
            .collect(),
        files: manifest_files,
        original_bytes,
        stored_bytes,
    };

    let manifest_offset = archive.stream_position()?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    archive.write_all(&manifest_bytes)?;
    archive.write_all(&manifest_offset.to_le_bytes())?;
    archive.write_all(&(manifest_bytes.len() as u64).to_le_bytes())?;
    archive.write_all(FOOTER_MAGIC)?;
    archive.flush()?;
    drop(archive);

    let read_back = read_archive_manifest(&partial_path)?;
    if read_back.files.is_empty() {
        bail!("Archive manifest validation failed: file list is empty.");
    }

    fs::rename(&partial_path, &archive_path).with_context(|| {
        format!(
            "failed to finalize archive {} -> {}",
            partial_path.display(),
            archive_path.display()
        )
    })?;

    let verification = verify_archive(&archive_path).with_context(|| {
        format!(
            "archive verification failed after writing {}",
            archive_path.display()
        )
    });
    if let Err(error) = verification {
        let _ = fs::remove_file(&archive_path);
        bail!(error);
    }

    Ok(BackupResult {
        archive_path,
        file_count: read_back.files.len(),
        original_bytes: read_back.original_bytes,
        stored_bytes: read_back.stored_bytes,
    })
}

pub fn read_archive_manifest(path: &Path) -> anyhow::Result<ArchiveManifest> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open archive {}", path.display()))?;
    let length = file.metadata()?.len();
    if length < (HEADER_MAGIC.len() + FOOTER_MAGIC.len() + 16) as u64 {
        bail!("Archive is too small to be a valid WinRehome archive.");
    }

    let mut header = [0_u8; 4];
    file.read_exact(&mut header)?;
    if &header != HEADER_MAGIC {
        bail!("Invalid WinRehome archive header.");
    }

    file.seek(SeekFrom::End(-20))?;
    let mut footer = [0_u8; 20];
    file.read_exact(&mut footer)?;
    if &footer[16..20] != FOOTER_MAGIC {
        bail!("Invalid WinRehome archive footer.");
    }

    let manifest_offset = u64::from_le_bytes(footer[0..8].try_into()?);
    let manifest_len = u64::from_le_bytes(footer[8..16].try_into()?);
    if manifest_offset + manifest_len > length.saturating_sub(20) {
        bail!("Manifest location is outside the archive bounds.");
    }

    file.seek(SeekFrom::Start(manifest_offset))?;
    let mut manifest_bytes = vec![0_u8; manifest_len as usize];
    file.read_exact(&mut manifest_bytes)?;
    let manifest: ArchiveManifest =
        serde_json::from_slice(&manifest_bytes).context("failed to decode archive manifest")?;
    if manifest.format_version != FORMAT_VERSION {
        bail!(
            "Unsupported WinRehome archive format version {} (expected {}).",
            manifest.format_version,
            FORMAT_VERSION
        );
    }
    Ok(manifest)
}

fn collect_pending_files(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
) -> anyhow::Result<Vec<PendingFile>> {
    let mut pending = Vec::new();
    let mut seen_sources = HashSet::new();
    let mut seen_archive_paths = HashSet::new();

    for root in preview
        .user_data_roots
        .iter()
        .filter(|root| selected_user_roots.contains(&path_key(&root.path)))
    {
        collect_path_files(
            &root.path,
            &format!(
                "user/{}/{}",
                sanitize_segment(&root.category),
                sanitize_segment(&root.label)
            ),
            "user_data",
            &mut seen_sources,
            &mut seen_archive_paths,
            &mut pending,
        )?;
    }

    for app in preview
        .portable_candidates
        .iter()
        .filter(|candidate| selected_portable_apps.contains(&path_key(&candidate.root_path)))
    {
        collect_path_files(
            &app.root_path,
            &format!("portable/{}", sanitize_segment(&app.display_name)),
            "portable_app",
            &mut seen_sources,
            &mut seen_archive_paths,
            &mut pending,
        )?;
    }

    for app in preview.installed_apps.iter().filter(|app| {
        selected_installed_app_dirs.contains(&app.selection_key()) && app.can_backup_files()
    }) {
        if let Some(install_location) = &app.install_location {
            collect_path_files(
                install_location,
                &installed_app_backup_root(app),
                "installed_app",
                &mut seen_sources,
                &mut seen_archive_paths,
                &mut pending,
            )?;
        }
    }

    Ok(pending)
}

fn installed_app_backup_root(app: &crate::models::InstalledAppRecord) -> String {
    format!(
        "installed/{}__{}__{}",
        sanitize_segment(&app.display_name),
        sanitize_segment(app.source),
        sanitize_segment(&app.uninstall_key)
    )
}

fn collect_path_files(
    source_root: &Path,
    archive_root: &str,
    entry_kind: &'static str,
    seen_sources: &mut HashSet<String>,
    seen_archive_paths: &mut HashSet<String>,
    pending: &mut Vec<PendingFile>,
) -> anyhow::Result<()> {
    if !source_root.exists() {
        return Ok(());
    }

    if source_root.is_file() {
        let source_key = source_root.display().to_string().to_lowercase();
        if seen_sources.insert(source_key) {
            let file_name = source_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            let archive_path = format!("{archive_root}/{}", sanitize_segment(file_name));
            let archive_key = archive_path.to_ascii_lowercase();
            if !seen_archive_paths.insert(archive_key) {
                bail!("duplicate archive entry path: {}", archive_path);
            }
            pending.push(PendingFile {
                source_path: source_root.to_path_buf(),
                archive_path,
                entry_kind,
            });
        }
        return Ok(());
    }

    let mut walker = WalkDir::new(source_root).into_iter();
    while let Some(entry) = walker.next() {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };

        if entry.depth() > 0 && should_exclude_path(entry.path()) {
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let relative = match entry.path().strip_prefix(source_root) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let relative_string = sanitize_relative_path(relative);
        let source_key = entry.path().display().to_string().to_lowercase();

        if seen_sources.insert(source_key) {
            let archive_path = format!("{archive_root}/{relative_string}");
            let archive_key = archive_path.to_ascii_lowercase();
            if !seen_archive_paths.insert(archive_key) {
                bail!("duplicate archive entry path: {}", archive_path);
            }
            pending.push(PendingFile {
                source_path: entry.path().to_path_buf(),
                archive_path,
                entry_kind,
            });
        }
    }

    Ok(())
}

fn write_file_entry(archive: &mut File, pending: PendingFile) -> anyhow::Result<ArchivedFileEntry> {
    let offset = archive.stream_position()?;
    let mut source = File::open(&pending.source_path)
        .with_context(|| format!("failed to open {}", pending.source_path.display()))?;
    let mut encoder = DeflateEncoder::new(
        CountingWriter::new(archive.try_clone()?),
        Compression::default(),
    );
    let mut hasher = Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut original_size = 0_u64;

    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        original_size += read as u64;
        hasher.update(&buffer[..read]);
        encoder.write_all(&buffer[..read])?;
    }

    let counting = encoder.finish()?;
    let stored_size = counting.written;

    archive.seek(SeekFrom::Start(offset + stored_size))?;

    Ok(ArchivedFileEntry {
        source_path: pending.source_path.display().to_string(),
        archive_path: pending.archive_path,
        entry_kind: pending.entry_kind.to_string(),
        offset,
        stored_size,
        original_size,
        crc32: hasher.finalize(),
    })
}

fn collect_selected_backup_source_dirs(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    for root in preview
        .user_data_roots
        .iter()
        .filter(|root| selected_user_roots.contains(&path_key(&root.path)))
    {
        if root.path.is_dir() {
            let key = path_key(&root.path);
            if seen.insert(key) {
                dirs.push(root.path.clone());
            }
        }
    }

    for app in preview
        .portable_candidates
        .iter()
        .filter(|candidate| selected_portable_apps.contains(&path_key(&candidate.root_path)))
    {
        if app.root_path.is_dir() {
            let key = path_key(&app.root_path);
            if seen.insert(key) {
                dirs.push(app.root_path.clone());
            }
        }
    }

    for app in preview.installed_apps.iter().filter(|app| {
        selected_installed_app_dirs.contains(&app.selection_key()) && app.can_backup_files()
    }) {
        if let Some(install_location) = &app.install_location {
            if install_location.is_dir() {
                let key = path_key(install_location);
                if seen.insert(key) {
                    dirs.push(install_location.clone());
                }
            }
        }
    }

    dirs
}

fn find_existing_file_in_ancestors(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors().skip(1) {
        if ancestor.exists() && ancestor.is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn sanitize_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            output.push('_');
        } else {
            output.push(ch);
        }
    }
    output.trim().trim_matches('.').to_string()
}

fn sanitize_relative_path(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(sanitize_segment)
        .filter(|part| !part.is_empty())
        .collect();

    if parts.is_empty() {
        "file".to_string()
    } else {
        parts.join("/")
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        ArchiveManifest, ArchivedFileEntry, FOOTER_MAGIC, FORMAT_VERSION, ManifestRoot,
        RestoreSelection, list_recent_archives_from_dirs, preview_backup_output,
        preview_restore_with_manifest, read_archive_manifest, restore_archive,
        restore_archive_with_selection, verify_archive,
    };
    use crate::models::{
        InstalledAppRecord, PathStats, PortableAppCandidate, PortableConfidence, UserDataRoot,
    };
    use crate::plan::BackupPreview;
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_backup_archive_at(
        preview: &BackupPreview,
        selected_user_roots: &HashSet<String>,
        selected_portable_apps: &HashSet<String>,
        output_dir: &std::path::Path,
    ) -> anyhow::Result<super::BackupResult> {
        super::create_backup_archive_at(
            preview,
            selected_user_roots,
            selected_portable_apps,
            &HashSet::new(),
            output_dir,
        )
    }

    #[test]
    fn writes_and_reads_archive_manifest() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-archive-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        let portable = root.join("PortableTool");
        fs::create_dir_all(&docs).expect("create docs");
        fs::create_dir_all(&portable).expect("create portable");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");
        fs::write(portable.join("Tool.exe"), b"exe").expect("write exe");
        fs::write(portable.join("tool.ini"), b"ini").expect("write ini");

        let docs_stats = PathStats {
            file_count: 1,
            total_bytes: 5,
        };
        let portable_stats = PathStats {
            file_count: 2,
            total_bytes: 6,
        };

        let preview = BackupPreview {
            installed_apps: vec![InstalledAppRecord {
                display_name: "Git".to_string(),
                source: "hklm-64",
                install_location: Some(PathBuf::from("C:\\Program Files\\Git")),
                install_stats: Some(PathStats::default()),
                uninstall_key: "Git_is1".to_string(),
            }],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                stats: portable_stats,
                reasons: vec!["2 executable(s) found".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: docs_stats,
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let selected_portable_apps = HashSet::from([portable.display().to_string().to_lowercase()]);

        let output_dir = root.join("out");
        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &selected_portable_apps,
            &output_dir,
        )
        .expect("create archive");
        let manifest = read_archive_manifest(&result.archive_path).expect("read manifest");

        assert_eq!(manifest.app_name, "WinRehome");
        assert_eq!(manifest.files.len(), 3);
        assert_eq!(manifest.selected_user_roots.len(), 1);
        assert_eq!(manifest.selected_portable_apps.len(), 1);
        assert!(result.archive_path.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recent_archives_can_merge_multiple_directories() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-recent-archives-{unique}"));
        let dir_a = root.join("A");
        let dir_b = root.join("B");
        fs::create_dir_all(&dir_a).expect("create dir a");
        fs::create_dir_all(&dir_b).expect("create dir b");

        let archive_a = dir_a.join("first.wrh");
        let archive_b = dir_b.join("second.wrh");
        fs::write(&archive_a, b"fake-archive-a").expect("write archive a");
        fs::write(&archive_b, b"fake-archive-b").expect("write archive b");

        let archives =
            list_recent_archives_from_dirs(&[dir_a.clone(), dir_b.clone(), dir_a.clone()], 10)
                .expect("list recent archives");

        assert_eq!(archives.len(), 2);
        assert!(archives.contains(&archive_a));
        assert!(archives.contains(&archive_b));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preflight_counts_existing_conflicts() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-preflight-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let result = create_backup_archive_at(
            &preview,
            &HashSet::from([docs.display().to_string().to_lowercase()]),
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");
        let manifest = read_archive_manifest(&result.archive_path).expect("read manifest");
        let restore_dir = root.join("restore");
        fs::create_dir_all(restore_dir.join("user/Personal Files/Documents"))
            .expect("create restore dir");
        fs::write(
            restore_dir.join("user/Personal Files/Documents/note.txt"),
            b"existing",
        )
        .expect("write existing restore file");

        let preview = preview_restore_with_manifest(
            &restore_dir,
            &manifest,
            &RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect("preview restore");

        assert_eq!(preview.selected_files, 1);
        assert_eq!(preview.new_files, 0);
        assert_eq!(preview.conflicting_files, 1);
        assert!(preview.new_examples.is_empty());
        assert_eq!(preview.conflict_examples.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preflight_counts_new_files_when_destination_is_empty() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-preflight-new-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let result = create_backup_archive_at(
            &preview,
            &HashSet::from([docs.display().to_string().to_lowercase()]),
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");
        let manifest = read_archive_manifest(&result.archive_path).expect("read manifest");
        let restore_dir = root.join("restore");

        let preview = preview_restore_with_manifest(
            &restore_dir,
            &manifest,
            &RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect("preview restore");

        assert_eq!(preview.selected_files, 1);
        assert_eq!(preview.new_files, 1);
        assert_eq!(preview.conflicting_files, 0);
        assert_eq!(preview.new_examples.len(), 1);
        assert!(preview.conflict_examples.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preflight_rejects_destination_that_is_file() {
        let manifest = ArchiveManifest {
            format_version: 1,
            created_at_unix: 1,
            app_name: "WinRehome".to_string(),
            app_version: "0.1.0".to_string(),
            installed_apps: vec![],
            selected_user_roots: vec![ManifestRoot {
                category: "Personal Files".to_string(),
                label: "Documents".to_string(),
                path: "C:\\Users\\Sunny\\Documents".to_string(),
                reason: "Test".to_string(),
            }],
            selected_portable_apps: vec![],
            files: vec![ArchivedFileEntry {
                source_path: "a".to_string(),
                archive_path: "user/Personal Files/Documents/note.txt".to_string(),
                entry_kind: "user_data".to_string(),
                offset: 0,
                stored_size: 1,
                original_size: 10,
                crc32: 1,
            }],
            original_bytes: 10,
            stored_bytes: 8,
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let destination_file =
            std::env::temp_dir().join(format!("winrehome-preflight-destination-{unique}.txt"));
        fs::write(&destination_file, b"not a directory").expect("write destination file");

        let error = preview_restore_with_manifest(
            &destination_file,
            &manifest,
            &RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect_err("file destination should be rejected");

        assert!(error.to_string().contains("existing file"));

        let _ = fs::remove_file(destination_file);
    }

    #[test]
    fn backup_preflight_rejects_output_inside_selected_source_dir() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-backup-overlap-{unique}"));
        let docs = root.join("Documents");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let error = preview_backup_output(
            &preview,
            &HashSet::from([docs.display().to_string().to_lowercase()]),
            &HashSet::new(),
            &HashSet::new(),
            &docs.join("Backups"),
        )
        .expect_err("output inside selected source should be rejected");

        assert!(
            error
                .to_string()
                .contains("overlaps selected source directory")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preflight_rejects_parent_path_blocked_by_file() {
        let manifest = ArchiveManifest {
            format_version: 1,
            created_at_unix: 1,
            app_name: "WinRehome".to_string(),
            app_version: "0.1.0".to_string(),
            installed_apps: vec![],
            selected_user_roots: vec![ManifestRoot {
                category: "Personal Files".to_string(),
                label: "Documents".to_string(),
                path: "C:\\Users\\Sunny\\Documents".to_string(),
                reason: "Test".to_string(),
            }],
            selected_portable_apps: vec![],
            files: vec![ArchivedFileEntry {
                source_path: "a".to_string(),
                archive_path: "user/Personal Files/Documents/note.txt".to_string(),
                entry_kind: "user_data".to_string(),
                offset: 0,
                stored_size: 1,
                original_size: 1,
                crc32: 1,
            }],
            original_bytes: 1,
            stored_bytes: 1,
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-blocker-{unique}"));
        fs::create_dir_all(root.join("user/Personal Files")).expect("create restore dirs");
        fs::write(root.join("user/Personal Files/Documents"), b"blocker")
            .expect("write blocker file");

        let error = preview_restore_with_manifest(
            &root,
            &manifest,
            &RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect_err("parent file blocker should be rejected");

        assert!(error.to_string().contains("blocked by existing file"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preflight_rejects_duplicate_output_paths() {
        let manifest = ArchiveManifest {
            format_version: 1,
            created_at_unix: 1,
            app_name: "WinRehome".to_string(),
            app_version: "0.1.0".to_string(),
            installed_apps: vec![],
            selected_user_roots: vec![ManifestRoot {
                category: "Personal Files".to_string(),
                label: "Documents".to_string(),
                path: "C:\\Users\\Sunny\\Documents".to_string(),
                reason: "Test".to_string(),
            }],
            selected_portable_apps: vec![],
            files: vec![
                ArchivedFileEntry {
                    source_path: "a".to_string(),
                    archive_path: "user/Personal Files/Documents/Note.txt".to_string(),
                    entry_kind: "user_data".to_string(),
                    offset: 0,
                    stored_size: 1,
                    original_size: 1,
                    crc32: 1,
                },
                ArchivedFileEntry {
                    source_path: "b".to_string(),
                    archive_path: "user/Personal Files/Documents/note.txt".to_string(),
                    entry_kind: "user_data".to_string(),
                    offset: 1,
                    stored_size: 1,
                    original_size: 1,
                    crc32: 2,
                },
            ],
            original_bytes: 2,
            stored_bytes: 2,
        };

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-duplicate-{unique}"));
        fs::create_dir_all(&root).expect("create restore dir");

        let error = preview_restore_with_manifest(
            &root,
            &manifest,
            &RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect_err("duplicate target paths should be rejected");

        assert!(error.to_string().contains("duplicated in archive"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restores_archive_contents() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-{unique}"));
        fs::create_dir_all(&root).expect("create restore test root");
        let docs = root.join("Docs");
        let portable = root.join("PortableTool");
        fs::create_dir_all(&docs).expect("create docs");
        fs::create_dir_all(&portable).expect("create portable");
        fs::write(docs.join("note.txt"), b"hello world").expect("write docs");
        fs::write(portable.join("Tool.exe"), b"exe-bytes").expect("write exe");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 9,
                },
                reasons: vec!["portable".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 11,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let selected_portable_apps = HashSet::from([portable.display().to_string().to_lowercase()]);
        let archive_dir = root.join("archive");
        let restore_dir = root.join("restore");

        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &selected_portable_apps,
            &archive_dir,
        )
        .expect("create archive");
        let restore = restore_archive(&result.archive_path, &restore_dir).expect("restore archive");

        assert_eq!(restore.restored_files, 2);
        assert_eq!(restore.skipped_existing_files, 0);
        assert_eq!(
            fs::read(restore_dir.join("user/Personal Files/Documents/note.txt"))
                .expect("read restored note"),
            b"hello world"
        );
        assert_eq!(
            fs::read(restore_dir.join("portable/PortableTool/Tool.exe"))
                .expect("read restored exe"),
            b"exe-bytes"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_refuses_to_overwrite_existing_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-conflict-{unique}"));
        fs::create_dir_all(&root).expect("create conflict test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let archive_dir = root.join("archive");
        let restore_dir = root.join("restore");
        fs::create_dir_all(restore_dir.join("user/Personal Files/Documents"))
            .expect("create restore docs");
        fs::write(
            restore_dir.join("user/Personal Files/Documents/note.txt"),
            b"existing",
        )
        .expect("write existing restored file");

        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &HashSet::new(),
            &archive_dir,
        )
        .expect("create archive");
        let error = restore_archive(&result.archive_path, &restore_dir)
            .expect_err("restore should refuse overwrite");

        assert!(error.to_string().contains("already exists"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_can_limit_to_user_data_only() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-filter-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        let portable = root.join("PortableTool");
        fs::create_dir_all(&docs).expect("create docs");
        fs::create_dir_all(&portable).expect("create portable");
        fs::write(docs.join("note.txt"), b"hello world").expect("write docs");
        fs::write(portable.join("Tool.exe"), b"exe-bytes").expect("write exe");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 9,
                },
                reasons: vec!["portable".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 11,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let selected_portable_apps = HashSet::from([portable.display().to_string().to_lowercase()]);
        let archive_dir = root.join("archive");
        let restore_dir = root.join("restore");

        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &selected_portable_apps,
            &archive_dir,
        )
        .expect("create archive");
        let restore = restore_archive_with_selection(
            &result.archive_path,
            &restore_dir,
            RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["user/Personal Files/Documents".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect("restore filtered archive");

        assert_eq!(restore.restored_files, 1);
        assert_eq!(restore.skipped_existing_files, 0);
        assert!(
            restore_dir
                .join("user/Personal Files/Documents/note.txt")
                .exists()
        );
        assert!(!restore_dir.join("portable/PortableTool/Tool.exe").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_can_limit_to_specific_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-restore-root-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        let portable = root.join("PortableTool");
        fs::create_dir_all(&docs).expect("create docs");
        fs::create_dir_all(&portable).expect("create portable");
        fs::write(docs.join("note.txt"), b"hello world").expect("write docs");
        fs::write(portable.join("Tool.exe"), b"exe-bytes").expect("write exe");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 9,
                },
                reasons: vec!["portable".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 11,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let selected_portable_apps = HashSet::from([portable.display().to_string().to_lowercase()]);
        let archive_dir = root.join("archive");
        let restore_dir = root.join("restore");

        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &selected_portable_apps,
            &archive_dir,
        )
        .expect("create archive");
        let restore = restore_archive_with_selection(
            &result.archive_path,
            &restore_dir,
            RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: true,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from(["portable/PortableTool".to_string()]),
                skip_existing_files: false,
            },
        )
        .expect("restore selected root");

        assert_eq!(restore.restored_files, 1);
        assert_eq!(restore.skipped_existing_files, 0);
        assert!(
            !restore_dir
                .join("user/Personal Files/Documents/note.txt")
                .exists()
        );
        assert!(restore_dir.join("portable/PortableTool/Tool.exe").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_can_skip_existing_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-skip-existing-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        let portable = root.join("PortableTool");
        fs::create_dir_all(&docs).expect("create docs");
        fs::create_dir_all(&portable).expect("create portable");
        fs::write(docs.join("note.txt"), b"hello world").expect("write docs");
        fs::write(portable.join("Tool.exe"), b"exe-bytes").expect("write exe");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 9,
                },
                reasons: vec!["portable".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 11,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let selected_portable_apps = HashSet::from([portable.display().to_string().to_lowercase()]);
        let archive_dir = root.join("archive");
        let restore_dir = root.join("restore");
        fs::create_dir_all(restore_dir.join("portable/PortableTool")).expect("create restore dir");
        fs::write(
            restore_dir.join("portable/PortableTool/Tool.exe"),
            b"existing",
        )
        .expect("write existing file");

        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &selected_portable_apps,
            &archive_dir,
        )
        .expect("create archive");
        let restore = restore_archive_with_selection(
            &result.archive_path,
            &restore_dir,
            RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: true,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::from([
                    "user/Personal Files/Documents".to_string(),
                    "portable/PortableTool".to_string(),
                ]),
                skip_existing_files: true,
            },
        )
        .expect("restore with skip existing");

        assert_eq!(restore.restored_files, 1);
        assert_eq!(restore.skipped_existing_files, 1);
        assert_eq!(
            fs::read(restore_dir.join("portable/PortableTool/Tool.exe"))
                .expect("read existing preserved file"),
            b"existing"
        );
        assert!(
            restore_dir
                .join("user/Personal Files/Documents/note.txt")
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_with_empty_selected_roots_restores_nothing() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-empty-selection-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");
        let error = restore_archive_with_selection(
            &result.archive_path,
            &root.join("restore"),
            RestoreSelection {
                restore_user_data: true,
                restore_portable_apps: false,
                restore_installed_app_dirs: false,
                selected_roots: HashSet::new(),
                skip_existing_files: false,
            },
        )
        .expect_err("empty selected roots should not restore");

        assert!(error.to_string().contains("does not contain any files"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verify_archive_confirms_written_contents() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-verify-{unique}"));
        fs::create_dir_all(&root).expect("create verify test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello verify").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 12,
                },
            }],
        };

        let selected_user_roots = HashSet::from([docs.display().to_string().to_lowercase()]);
        let result = create_backup_archive_at(
            &preview,
            &selected_user_roots,
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");
        let verification = verify_archive(&result.archive_path).expect("verify archive");

        assert_eq!(verification.verified_files, 1);
        assert_eq!(verification.verified_bytes, 12);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsupported_archive_format_version() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-format-version-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let archive = create_backup_archive_at(
            &preview,
            &HashSet::from([docs.display().to_string().to_lowercase()]),
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");

        let mut bytes = fs::read(&archive.archive_path).expect("read archive bytes");
        let footer = &bytes[bytes.len() - 20..];
        let manifest_offset = u64::from_le_bytes(footer[0..8].try_into().expect("footer offset"));
        let manifest_len = u64::from_le_bytes(footer[8..16].try_into().expect("footer length"));
        let start = manifest_offset as usize;
        let end = start + manifest_len as usize;
        let manifest_bytes = &bytes[start..end];
        let mut manifest: Value =
            serde_json::from_slice(manifest_bytes).expect("decode stored manifest");
        manifest["format_version"] = json!(FORMAT_VERSION + 1);
        let patched_manifest =
            serde_json::to_vec_pretty(&manifest).expect("encode patched manifest");
        assert_eq!(patched_manifest.len(), manifest_bytes.len());
        bytes[start..end].copy_from_slice(&patched_manifest);
        fs::write(&archive.archive_path, &bytes).expect("rewrite archive");

        let error = read_archive_manifest(&archive.archive_path)
            .expect_err("unsupported version should fail");
        assert!(
            error
                .to_string()
                .contains("Unsupported WinRehome archive format version")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_restore_path_that_escapes_destination_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("winrehome-escape-path-{unique}"));
        fs::create_dir_all(&root).expect("create test root");
        let docs = root.join("Docs");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(docs.join("note.txt"), b"hello").expect("write docs");

        let preview = BackupPreview {
            installed_apps: vec![],
            portable_candidates: vec![],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: docs.clone(),
                reason: "Test documents".into(),
                stats: PathStats {
                    file_count: 1,
                    total_bytes: 5,
                },
            }],
        };

        let archive = create_backup_archive_at(
            &preview,
            &HashSet::from([docs.display().to_string().to_lowercase()]),
            &HashSet::new(),
            &root.join("archive"),
        )
        .expect("create archive");

        let bytes = fs::read(&archive.archive_path).expect("read archive bytes");
        let footer = &bytes[bytes.len() - 20..];
        let manifest_offset = u64::from_le_bytes(footer[0..8].try_into().expect("footer offset"));
        let manifest_len = u64::from_le_bytes(footer[8..16].try_into().expect("footer length"));
        let start = manifest_offset as usize;
        let end = start + manifest_len as usize;
        let manifest_bytes = &bytes[start..end];
        let mut manifest: Value =
            serde_json::from_slice(manifest_bytes).expect("decode stored manifest");
        manifest["files"][0]["archive_path"] =
            json!("user/Personal Files/Documents/../../escaped/note.txt");
        let patched_manifest =
            serde_json::to_vec_pretty(&manifest).expect("encode patched manifest");

        let mut patched_bytes = Vec::with_capacity(start + patched_manifest.len() + 20);
        patched_bytes.extend_from_slice(&bytes[..start]);
        patched_bytes.extend_from_slice(&patched_manifest);
        patched_bytes.extend_from_slice(&manifest_offset.to_le_bytes());
        patched_bytes.extend_from_slice(&(patched_manifest.len() as u64).to_le_bytes());
        patched_bytes.extend_from_slice(FOOTER_MAGIC);
        fs::write(&archive.archive_path, &patched_bytes).expect("rewrite archive");

        let restore_root = root.join("restore");
        let error = restore_archive(&archive.archive_path, &restore_root)
            .expect_err("restore should reject escaping archive paths");
        assert!(error.to_string().contains("escapes restore root"));
        assert!(!root.join("escaped").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_footer() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let file = std::env::temp_dir().join(format!("winrehome-invalid-{unique}.wrh"));
        fs::write(&file, b"not-an-archive").expect("write invalid archive");

        let error = read_archive_manifest(&file).expect_err("invalid archive should fail");
        assert!(error.to_string().contains("too small") || error.to_string().contains("header"));

        let _ = fs::remove_file(file);
    }
}
