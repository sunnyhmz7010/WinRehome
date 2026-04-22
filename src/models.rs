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
    pub label: &'static str,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ExclusionRule {
    pub label: &'static str,
    pub pattern: &'static str,
}
