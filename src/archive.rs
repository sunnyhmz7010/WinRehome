use crate::plan::{path_key, should_exclude_path};
use anyhow::{Context, bail};
use crc32fast::Hasher;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestInstalledApp {
    pub display_name: String,
    pub source: String,
    pub install_location: Option<String>,
    pub uninstall_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestRoot {
    pub category: String,
    pub label: String,
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestPortableApp {
    pub display_name: String,
    pub root_path: String,
    pub main_executable: String,
    pub confidence: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
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

pub fn create_backup_archive(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
) -> anyhow::Result<BackupResult> {
    let output_dir = default_output_dir()?;
    create_backup_archive_at(
        preview,
        selected_user_roots,
        selected_portable_apps,
        &output_dir,
    )
}

fn create_backup_archive_at(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    output_dir: &Path,
) -> anyhow::Result<BackupResult> {
    let pending_files =
        collect_pending_files(preview, selected_user_roots, selected_portable_apps)?;
    if pending_files.is_empty() {
        bail!("No files are selected for backup.");
    }

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
                display_name: app.display_name.clone(),
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
    Ok(manifest)
}

fn collect_pending_files(
    preview: &crate::plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
) -> anyhow::Result<Vec<PendingFile>> {
    let mut pending = Vec::new();
    let mut seen_sources = HashSet::new();

    for root in preview
        .user_data_roots
        .iter()
        .filter(|root| selected_user_roots.contains(&path_key(&root.path)))
    {
        collect_path_files(
            &root.path,
            &format!(
                "user/{}/{}",
                sanitize_segment(root.category),
                sanitize_segment(root.label)
            ),
            "user_data",
            &mut seen_sources,
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
            &mut pending,
        )?;
    }

    Ok(pending)
}

fn collect_path_files(
    source_root: &Path,
    archive_root: &str,
    entry_kind: &'static str,
    seen_sources: &mut HashSet<String>,
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
            pending.push(PendingFile {
                source_path: source_root.to_path_buf(),
                archive_path: format!("{archive_root}/{}", sanitize_segment(file_name)),
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
            pending.push(PendingFile {
                source_path: entry.path().to_path_buf(),
                archive_path: format!("{archive_root}/{relative_string}"),
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

fn default_output_dir() -> anyhow::Result<PathBuf> {
    if let Some(profile) = env::var_os("USERPROFILE") {
        let desktop = PathBuf::from(profile)
            .join("Desktop")
            .join("WinRehome Backups");
        return Ok(desktop);
    }

    Ok(env::current_dir()?.join("WinRehome Backups"))
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
    use super::{ArchiveManifest, create_backup_archive_at, read_archive_manifest};
    use crate::models::{
        ExclusionRule, InstalledAppRecord, PathStats, PortableAppCandidate, PortableConfidence,
        UserDataRoot,
    };
    use crate::plan::BackupPreview;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
                uninstall_key: "Git_is1".to_string(),
            }],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: portable.clone(),
                main_executable: portable.join("Tool.exe"),
                confidence: PortableConfidence::High,
                default_selected: true,
                stats: portable_stats,
                reasons: vec!["2 executable(s) found".to_string()],
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files",
                label: "Documents",
                path: docs.clone(),
                reason: "Test documents",
                default_selected: true,
                stats: docs_stats,
            }],
            exclusion_rules: vec![ExclusionRule {
                label: "Logs",
                pattern: "Logs",
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

    #[allow(dead_code)]
    fn _assert_manifest_send_sync(_: &ArchiveManifest) {}
}
