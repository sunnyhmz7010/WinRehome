use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct InstalledAppRecord {
    pub display_name: String,
    pub source: &'static str,
    pub install_location: Option<PathBuf>,
    pub uninstall_key: String,
}

#[derive(Debug, Clone)]
pub struct PortableAppCandidate {
    pub display_name: String,
    pub root_path: PathBuf,
    pub main_executable: PathBuf,
    pub confidence: PortableConfidence,
    pub default_selected: bool,
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
    pub category: &'static str,
    pub label: &'static str,
    pub path: PathBuf,
    pub reason: &'static str,
    pub default_selected: bool,
    pub stats: PathStats,
}

#[derive(Debug, Clone)]
pub struct ExclusionRule {
    pub label: &'static str,
    pub pattern: &'static str,
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
