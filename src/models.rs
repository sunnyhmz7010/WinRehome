use std::borrow::Cow;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct InstalledAppRecord {
    pub display_name: String,
    pub source: &'static str,
    pub install_location: Option<PathBuf>,
    pub install_stats: Option<PathStats>,
    pub uninstall_key: String,
}

impl InstalledAppRecord {
    pub fn selection_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.source.to_lowercase(),
            self.uninstall_key.to_lowercase(),
            self.display_name.to_lowercase()
        )
    }

    pub fn can_backup_files(&self) -> bool {
        self.install_location.is_some() && self.install_stats.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct PortableAppCandidate {
    pub display_name: String,
    pub root_path: PathBuf,
    pub main_executable: PathBuf,
    pub confidence: PortableConfidence,
    pub stats: PathStats,
    pub reasons: Vec<String>,
}

impl PortableAppCandidate {
    pub fn confidence_label(&self) -> &'static str {
        match self.confidence {
            PortableConfidence::High => "high",
            PortableConfidence::Medium => "medium",
            PortableConfidence::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PortableConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone)]
pub struct UserDataRoot {
    pub category: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub path: PathBuf,
    pub reason: Cow<'static, str>,
    pub stats: PathStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PathStats {
    pub file_count: u64,
    pub total_bytes: u64,
}

impl PathStats {
    pub fn add(&mut self, other: PathStats) {
        self.file_count += other.file_count;
        self.total_bytes += other.total_bytes;
    }
}
