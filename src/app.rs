use crate::{archive, config, plan};
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const GITHUB_REPO_URL: &str = "https://github.com/sunnyhmz7010/WinRehome";
const FEEDBACK_URL: &str = "https://github.com/sunnyhmz7010/WinRehome/issues";

#[derive(Debug, Clone)]
struct LoadedArchive {
    path: PathBuf,
    manifest: archive::ArchiveManifest,
}

#[derive(Debug, Clone)]
struct InstalledAppExportRow {
    display_name: String,
    source: String,
    install_location: Option<String>,
    uninstall_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundTaskKind {
    Scan,
    Restore,
}

#[derive(Debug, Clone)]
struct BackgroundTaskProgress {
    kind: BackgroundTaskKind,
    title: String,
    detail: String,
    fraction: f32,
}

#[derive(Debug)]
enum BackgroundTaskMessage {
    Progress(BackgroundTaskProgress),
    ScanFinished(Result<plan::BackupPreview, String>),
    RestoreFinished(Result<archive::RestoreResult, String>),
}

#[derive(Debug)]
enum BackgroundTaskCompletion {
    Scan(Result<plan::BackupPreview, String>),
    Restore(Result<archive::RestoreResult, String>),
}

#[derive(Debug)]
struct BackgroundTaskState {
    kind: BackgroundTaskKind,
    progress: BackgroundTaskProgress,
    receiver: Receiver<BackgroundTaskMessage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RestorePreviewSummary {
    selected_root_count: usize,
    selected_user_root_count: usize,
    selected_portable_app_count: usize,
    selected_installed_app_dir_count: usize,
    selected_file_count: usize,
    selected_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ScanUserRootSummary {
    filtered_count: usize,
    visible_keys: Vec<String>,
    visible_selected_count: usize,
    visible_unselected_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RestoreRootListSummary {
    filtered_count: usize,
    visible_keys: Vec<String>,
    visible_selected_count: usize,
    visible_unselected_count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WorkspaceView {
    #[default]
    Overview,
    ScanPlan,
    Restore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum BackupWorkflowPage {
    #[default]
    ScanScope,
    UserData,
    PortableApps,
    InstalledApps,
    Output,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RestoreSection {
    InstalledApps,
    #[default]
    RestoreScope,
    RestoreAction,
}

#[derive(Debug, Clone)]
struct ScanRootEntry {
    path: String,
}

#[derive(Default)]
pub struct WinRehomeApp {
    active_workspace: WorkspaceView,
    backup_page: BackupWorkflowPage,
    restore_section: RestoreSection,
    preview: Option<plan::BackupPreview>,
    scan_filter: String,
    restore_filter: String,
    restore_scope_only_unselected_roots: bool,
    restore_inventory_filter: String,
    selected_user_roots: HashSet<String>,
    selected_portable_apps: HashSet<String>,
    selected_installed_app_dirs: HashSet<String>,
    scan_roots: Vec<ScanRootEntry>,
    excluded_scan_roots: Vec<ScanRootEntry>,
    backup_output_input: String,
    archive_path_input: String,
    restore_destination_input: String,
    restore_user_data: bool,
    restore_portable_apps: bool,
    restore_installed_app_dirs: bool,
    selected_restore_roots: HashSet<String>,
    skip_existing_restore_files: bool,
    remember_window_geometry: bool,
    last_window_geometry: Option<config::SavedWindowGeometry>,
    recent_archives: Vec<PathBuf>,
    loaded_archive: Option<LoadedArchive>,
    last_archive: Option<archive::BackupResult>,
    last_restore: Option<archive::RestoreResult>,
    last_verification: Option<archive::VerificationResult>,
    last_notice: Option<String>,
    last_error: Option<String>,
    background_task: Option<BackgroundTaskState>,
}

impl WinRehomeApp {
    pub fn new() -> Self {
        let mut app = Self {
            scan_roots: default_scan_root_entries(),
            restore_user_data: true,
            restore_portable_apps: true,
            restore_installed_app_dirs: true,
            remember_window_geometry: true,
            ..Self::default()
        };

        if let Ok(Some(saved)) = config::load_config() {
            let saved_restore_user_data = saved.restore_user_data;
            let saved_restore_portable_apps = saved.restore_portable_apps;
            let saved_restore_installed_app_dirs = saved.restore_installed_app_dirs;
            let saved_skip_existing_restore_files = saved.skip_existing_restore_files;
            let saved_selected_restore_roots = saved.selected_restore_roots.clone();

            app.selected_user_roots = config::normalize_existing_paths(&saved.selected_user_roots);
            app.selected_portable_apps =
                config::normalize_existing_paths(&saved.selected_portable_apps);
            app.selected_installed_app_dirs = saved.selected_installed_app_dirs;
            if !saved.scan_roots.is_empty() {
                app.scan_roots = saved
                    .scan_roots
                    .into_iter()
                    .filter(|entry| entry.enabled)
                    .map(|entry| ScanRootEntry { path: entry.path })
                    .collect();
            }
            if !saved.excluded_scan_roots.is_empty() {
                app.excluded_scan_roots = saved
                    .excluded_scan_roots
                    .into_iter()
                    .filter(|entry| entry.enabled)
                    .map(|entry| ScanRootEntry { path: entry.path })
                    .collect();
            }
            app.backup_output_input = saved.last_backup_output_dir.unwrap_or_default();
            app.archive_path_input = saved.last_archive_path.unwrap_or_default();
            app.restore_destination_input = saved.last_restore_destination.unwrap_or_default();
            app.restore_user_data = saved_restore_user_data;
            app.restore_portable_apps = saved_restore_portable_apps;
            app.restore_installed_app_dirs = saved_restore_installed_app_dirs;
            app.selected_restore_roots = saved_selected_restore_roots.clone();
            app.skip_existing_restore_files = saved_skip_existing_restore_files;
            app.remember_window_geometry = saved.remember_window_geometry;
            app.last_window_geometry = saved.last_window_geometry.filter(|state| state.is_valid());

            if !app.archive_path_input.trim().is_empty() {
                let saved_restore_destination = app.restore_destination_input.clone();
                let path = PathBuf::from(app.archive_path_input.trim());
                if path.exists() {
                    app.load_archive_from_path(path);
                    app.restore_user_data = saved_restore_user_data;
                    app.restore_portable_apps = saved_restore_portable_apps;
                    app.restore_installed_app_dirs = saved_restore_installed_app_dirs;
                    app.skip_existing_restore_files = saved_skip_existing_restore_files;
                    app.selected_restore_roots = retained_restore_roots(
                        app.loaded_archive.as_ref(),
                        &saved_selected_restore_roots,
                    );
                    if !saved_restore_destination.trim().is_empty() {
                        app.restore_destination_input = saved_restore_destination;
                    }
                    let _ = app.persist_config();
                }
            }
        }

        if app.backup_output_input.trim().is_empty() {
            app.backup_output_input = archive::default_output_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
        }

        app.refresh_recent_archives();
        app
    }

    fn load_preview(&mut self, preview: plan::BackupPreview) {
        self.selected_user_roots.clear();
        self.selected_portable_apps.clear();
        self.selected_installed_app_dirs.clear();
        self.backup_page = first_available_backup_page(&preview);
        self.preview = Some(preview);
        self.scan_filter.clear();
        self.active_workspace = WorkspaceView::ScanPlan;
        self.last_notice = None;
        self.last_error = None;
    }

    fn configured_scan_roots(&self) -> Vec<PathBuf> {
        configured_path_entries(&self.scan_roots)
    }

    fn configured_excluded_scan_roots(&self) -> Vec<PathBuf> {
        configured_path_entries(&self.excluded_scan_roots)
    }

    fn start_scan_preview(&mut self) {
        let scan_roots = self.configured_scan_roots();
        let excluded_scan_roots = self.configured_excluded_scan_roots();
        if scan_roots.is_empty() {
            self.last_archive = None;
            self.last_error = Some("请先至少保留一个启用的扫描路径。".to_string());
            self.last_notice = None;
            return;
        }

        if self.background_task.is_some() {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        self.background_task = Some(BackgroundTaskState {
            kind: BackgroundTaskKind::Scan,
            progress: BackgroundTaskProgress {
                kind: BackgroundTaskKind::Scan,
                title: "正在扫描当前系统".to_string(),
                detail: "准备读取扫描范围...".to_string(),
                fraction: 0.0,
            },
            receiver,
        });
        self.last_archive = None;
        self.last_error = None;
        self.last_notice = None;

        thread::spawn(move || {
            let custom_user_roots = Vec::new();
            let result = plan::build_preview_for_scan_roots_with_excludes_and_progress(
                &scan_roots,
                &excluded_scan_roots,
                &custom_user_roots,
                |progress| {
                    let _ = sender.send(BackgroundTaskMessage::Progress(BackgroundTaskProgress {
                        kind: BackgroundTaskKind::Scan,
                        title: "正在扫描当前系统".to_string(),
                        detail: format!("{}：{}", progress.stage, progress.detail),
                        fraction: progress.fraction,
                    }));
                },
            )
            .map_err(|error| error.to_string());
            let _ = sender.send(BackgroundTaskMessage::ScanFinished(result));
        });
    }

    fn persist_config(&self) -> anyhow::Result<PathBuf> {
        config::save_config(&config::AppConfig {
            selected_user_roots: self.selected_user_roots.clone(),
            selected_portable_apps: self.selected_portable_apps.clone(),
            selected_installed_app_dirs: self.selected_installed_app_dirs.clone(),
            scan_roots: saved_path_entries(&self.scan_roots),
            excluded_scan_roots: saved_path_entries(&self.excluded_scan_roots),
            custom_user_roots: Vec::new(),
            last_backup_output_dir: (!self.backup_output_input.trim().is_empty())
                .then(|| self.backup_output_input.trim().to_string()),
            last_archive_path: (!self.archive_path_input.trim().is_empty())
                .then(|| self.archive_path_input.trim().to_string()),
            last_restore_destination: (!self.restore_destination_input.trim().is_empty())
                .then(|| self.restore_destination_input.trim().to_string()),
            restore_user_data: self.restore_user_data,
            restore_portable_apps: self.restore_portable_apps,
            restore_installed_app_dirs: self.restore_installed_app_dirs,
            selected_restore_roots: self.selected_restore_roots.clone(),
            skip_existing_restore_files: self.skip_existing_restore_files,
            remember_window_geometry: self.remember_window_geometry,
            last_window_geometry: self.last_window_geometry.clone(),
        })
    }

    fn capture_window_geometry(&mut self, ctx: &egui::Context) {
        let viewport = ctx.input(|input| input.viewport().clone());
        if viewport.minimized == Some(true) {
            return;
        }

        let Some(outer_rect) = viewport.outer_rect else {
            return;
        };
        let Some(inner_rect) = viewport.inner_rect else {
            return;
        };

        let geometry = config::SavedWindowGeometry {
            x: outer_rect.min.x,
            y: outer_rect.min.y,
            width: inner_rect.width(),
            height: inner_rect.height(),
            maximized: viewport.maximized.unwrap_or(false),
        };
        if geometry.is_valid() {
            self.last_window_geometry = Some(geometry);
        }
    }

    fn start_restore_task(
        &mut self,
        archive_path: PathBuf,
        destination: PathBuf,
        selection: archive::RestoreSelection,
    ) {
        if self.background_task.is_some() {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        self.background_task = Some(BackgroundTaskState {
            kind: BackgroundTaskKind::Restore,
            progress: BackgroundTaskProgress {
                kind: BackgroundTaskKind::Restore,
                title: "正在恢复备份".to_string(),
                detail: "准备校验恢复内容...".to_string(),
                fraction: 0.0,
            },
            receiver,
        });
        self.last_restore = None;
        self.last_error = None;
        self.last_notice = None;

        thread::spawn(move || {
            let result = archive::restore_archive_with_selection_and_progress(
                &archive_path,
                &destination,
                selection,
                |progress| {
                    let percent = if progress.total_files == 0 {
                        0.0
                    } else {
                        progress.processed_files as f32 / progress.total_files as f32
                    };
                    let detail = if progress.current_path.trim().is_empty() {
                        format!(
                            "已处理 {}/{} 个文件",
                            progress.processed_files, progress.total_files
                        )
                    } else {
                        format!(
                            "已处理 {}/{} 个文件\n{}",
                            progress.processed_files, progress.total_files, progress.current_path
                        )
                    };
                    let _ = sender.send(BackgroundTaskMessage::Progress(BackgroundTaskProgress {
                        kind: BackgroundTaskKind::Restore,
                        title: "正在恢复备份".to_string(),
                        detail,
                        fraction: percent,
                    }));
                },
            )
            .map_err(|error| present_restore_error(&error.to_string()));
            let _ = sender.send(BackgroundTaskMessage::RestoreFinished(result));
        });
    }

    fn poll_background_task(&mut self, ctx: &egui::Context) {
        let mut completed: Option<BackgroundTaskCompletion> = None;

        if let Some(task) = &mut self.background_task {
            while let Ok(message) = task.receiver.try_recv() {
                match message {
                    BackgroundTaskMessage::Progress(progress) => {
                        task.progress = progress;
                    }
                    BackgroundTaskMessage::ScanFinished(result) => {
                        completed = Some(BackgroundTaskCompletion::Scan(result));
                    }
                    BackgroundTaskMessage::RestoreFinished(result) => {
                        completed = Some(BackgroundTaskCompletion::Restore(result));
                    }
                }
            }

            if completed.is_none() {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }

        if let Some(completed) = completed {
            self.background_task = None;
            match completed {
                BackgroundTaskCompletion::Scan(scan_result) => match scan_result {
                    Ok(preview) => self.load_preview(preview),
                    Err(error) => {
                        self.last_archive = None;
                        self.last_error = Some(error);
                        self.last_notice = None;
                    }
                },
                BackgroundTaskCompletion::Restore(result) => match result {
                    Ok(restore) => {
                        self.last_restore = Some(restore);
                        self.last_verification = None;
                        self.last_notice = None;
                        self.last_error = None;
                        let _ = self.persist_config();
                    }
                    Err(error) => {
                        self.last_restore = None;
                        self.last_error = Some(error);
                        self.last_notice = None;
                    }
                },
            }
        }
    }

    fn load_archive_from_path(&mut self, path: PathBuf) {
        match archive::read_archive_manifest(&path) {
            Ok(manifest) => {
                let previous_archive_path = self
                    .loaded_archive
                    .as_ref()
                    .map(|archive| archive.path.clone())
                    .or_else(|| {
                        (!self.archive_path_input.trim().is_empty())
                            .then(|| PathBuf::from(self.archive_path_input.trim()))
                    });
                let previous_destination = self.restore_destination_input.clone();
                let previous_restore_filter = self.restore_filter.clone();
                let previous_restore_inventory_filter = self.restore_inventory_filter.clone();
                let previous_restore_section = self.restore_section;
                let previous_restore_user_data = self.restore_user_data;
                let previous_restore_portable_apps = self.restore_portable_apps;
                let previous_restore_installed_app_dirs = self.restore_installed_app_dirs;
                let previous_skip_existing_restore_files = self.skip_existing_restore_files;
                let previous_restore_scope_only_unselected =
                    self.restore_scope_only_unselected_roots;
                let previous_restore_roots = self.selected_restore_roots.clone();
                let same_archive_reload = previous_archive_path
                    .as_deref()
                    .map(|previous| same_archive_path(previous, &path))
                    .unwrap_or(false);
                self.restore_destination_input = restore_destination_for_loaded_archive(
                    &path,
                    same_archive_reload,
                    &previous_destination,
                );
                self.archive_path_input = path.display().to_string();
                let loaded = LoadedArchive { path, manifest };
                self.selected_restore_roots = restore_roots_for_loaded_archive(
                    &loaded,
                    previous_archive_path.as_deref(),
                    &previous_restore_roots,
                );
                self.loaded_archive = Some(loaded);
                self.restore_filter = restore_text_filter_for_loaded_archive(
                    same_archive_reload,
                    &previous_restore_filter,
                );
                self.restore_inventory_filter = restore_text_filter_for_loaded_archive(
                    same_archive_reload,
                    &previous_restore_inventory_filter,
                );
                self.restore_section = restore_section_for_loaded_archive(
                    same_archive_reload,
                    previous_restore_section,
                );
                self.active_workspace = WorkspaceView::Restore;
                let restored_flags = restore_flags_for_loaded_archive(
                    same_archive_reload,
                    previous_restore_user_data,
                    previous_restore_portable_apps,
                    previous_restore_installed_app_dirs,
                    previous_skip_existing_restore_files,
                );
                self.restore_user_data = restored_flags.0;
                self.restore_portable_apps = restored_flags.1;
                self.restore_installed_app_dirs = restored_flags.2;
                self.skip_existing_restore_files = restored_flags.3;
                self.restore_scope_only_unselected_roots = if same_archive_reload {
                    previous_restore_scope_only_unselected
                } else {
                    false
                };
                self.last_verification = None;
                self.last_restore = None;
                self.last_notice = None;
                self.last_error = None;
                self.refresh_recent_archives();
                let _ = self.persist_config();
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.last_notice = None;
            }
        }
    }

    fn refresh_recent_archives(&mut self) {
        self.recent_archives =
            archive::list_recent_archives_from_dirs(&self.recent_archive_search_dirs(), 8)
                .unwrap_or_default();
    }

    fn recent_archive_search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Ok(path) = archive::default_output_dir() {
            dirs.push(path);
        }
        if let Some(path) = optional_dir_from_input(&self.backup_output_input) {
            dirs.push(path);
        }
        if let Some(path) = optional_parent_dir_from_input(&self.archive_path_input) {
            dirs.push(path);
        }
        if let Some(result) = &self.last_archive {
            if let Some(parent) = result.archive_path.parent() {
                dirs.push(parent.to_path_buf());
            }
        }
        if let Some(loaded) = &self.loaded_archive {
            if let Some(parent) = loaded.path.parent() {
                dirs.push(parent.to_path_buf());
            }
        }

        dedupe_dirs(dirs)
    }
}

pub fn configure_egui(ctx: &egui::Context) {
    if let Some(font_bytes) = load_windows_cjk_font() {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "winrehome-cjk".to_string(),
            FontData::from_owned(font_bytes).into(),
        );
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "winrehome-cjk".to_string());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("winrehome-cjk".to_string());
        ctx.set_fonts(fonts);
    }

    configure_visual_style(ctx);
}

fn configure_visual_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.menu_margin = egui::Margin::same(12);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.indent = 18.0;

    style.visuals = {
        let mut visuals = egui::Visuals::light();
        visuals.override_text_color = Some(Color32::from_rgb(28, 34, 43));
        visuals.panel_fill = Color32::from_rgb(245, 247, 250);
        visuals.extreme_bg_color = Color32::from_rgb(234, 239, 246);
        visuals.window_fill = Color32::from_rgb(252, 253, 255);
        visuals.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(205, 213, 224));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(252, 253, 255);
        visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(214, 220, 230));
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(14);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(247, 249, 252);
        visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(192, 203, 219));
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(10);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(237, 243, 251);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(96, 134, 188));
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(10);
        visuals.widgets.active.bg_fill = Color32::from_rgb(0, 103, 192);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0, 79, 150));
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5, Color32::WHITE);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(10);
        visuals.selection.bg_fill = Color32::from_rgb(190, 216, 247);
        visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(0, 94, 177));
        visuals.hyperlink_color = Color32::from_rgb(0, 94, 177);
        visuals
    };

    ctx.set_style(style);
}

fn load_windows_cjk_font() -> Option<Vec<u8>> {
    const CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyhl.ttc",
        "C:\\Windows\\Fonts\\msyhbd.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsunb.ttf",
    ];

    for path in CANDIDATES {
        if let Ok(bytes) = fs::read(path) {
            return Some(bytes);
        }
    }

    None
}

impl eframe::App for WinRehomeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.capture_window_geometry(ctx);
        self.poll_background_task(ctx);

        let has_preview = self.preview.is_some();
        let has_loaded_archive = self.loaded_archive.is_some();
        let resolved_workspace =
            resolved_workspace(self.active_workspace, has_preview, has_loaded_archive);
        self.active_workspace = resolved_workspace;

        egui::TopBottomPanel::top("app_header")
            .exact_height(66.0)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(252, 253, 255))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(197, 208, 222)))
                    .inner_margin(egui::Margin::symmetric(20, 12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("WinRehome")
                                    .size(22.0)
                                    .strong()
                                    .color(Color32::from_rgb(32, 39, 49)),
                            );
                            ui.add_space(18.0);
                            workspace_button(
                                ui,
                                &mut self.active_workspace,
                                WorkspaceView::Overview,
                                "首页",
                                true,
                            );
                            workspace_button(
                                ui,
                                &mut self.active_workspace,
                                WorkspaceView::ScanPlan,
                                "创建备份",
                                true,
                            );
                            workspace_button(
                                ui,
                                &mut self.active_workspace,
                                WorkspaceView::Restore,
                                "恢复备份",
                                true,
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |_ui| {},
                            );
                        });
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let content_max_width = if matches!(
                resolved_workspace,
                WorkspaceView::ScanPlan | WorkspaceView::Restore
            ) {
                1560.0
            } else {
                1040.0
            };
            let use_main_scroll = matches!(resolved_workspace, WorkspaceView::Restore)
                || matches!(resolved_workspace, WorkspaceView::ScanPlan)
                    && self.backup_page != BackupWorkflowPage::ScanScope;
            let mut render_main = |ui: &mut egui::Ui| {
                centered_content(ui, content_max_width, |ui| {
                show_feedback_banners(ui, self);
                ui.add_space(12.0);

                if matches!(resolved_workspace, WorkspaceView::Overview) {
                    ui.add_space(24.0);
                }

                        if matches!(resolved_workspace, WorkspaceView::Overview) {
                            render_overview_card(ui, self);
                        }

                        if matches!(resolved_workspace, WorkspaceView::ScanPlan) {
                        if let Some(preview) = self.preview.clone() {
                            let visible_portable_keys: Vec<String> = preview
                                .portable_candidates
                                .iter()
                                .filter_map(|item| {
                                    let root_path = item.root_path.display().to_string();
                                    let main_executable = item.main_executable.display().to_string();
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &item.display_name,
                                            &root_path,
                                            &main_executable,
                                            item.confidence_label(),
                                        ],
                                    )
                                    .then(|| plan::path_key(&item.root_path))
                                })
                                .collect();
                            let scan_user_root_summary = summarize_scan_user_roots(
                                &preview.user_data_roots,
                                &self.scan_filter,
                                &self.selected_user_roots,
                            );
                            let filtered_installed_count = preview
                                .installed_apps
                                .iter()
                                .filter(|app| {
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &app.display_name,
                                            app.source,
                                            &app.uninstall_key,
                                            &app
                                                .install_location
                                                .as_ref()
                                                .map(|path| path.display().to_string())
                                                .unwrap_or_default(),
                                        ],
                                    )
                                })
                                .count();
                            let filtered_portable_count = preview
                                .portable_candidates
                                .iter()
                                .filter(|item| {
                                    let root_path = item.root_path.display().to_string();
                                    let main_executable = item.main_executable.display().to_string();
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &item.display_name,
                                            &root_path,
                                            &main_executable,
                                            item.confidence_label(),
                                        ],
                                    )
                                })
                                .count();
                            let summary = preview.summarize_selection(
                                &self.selected_user_roots,
                                &self.selected_portable_apps,
                                &self.selected_installed_app_dirs,
                            );
                            let backup_preflight = preview_backup_output_directory(
                                &preview,
                                &self.selected_user_roots,
                                &self.selected_portable_apps,
                                &self.selected_installed_app_dirs,
                                &self.backup_output_input,
                            );

                            let filtered_scan_apps: Vec<InstalledAppExportRow> = preview
                                .installed_apps
                                .iter()
                                .filter_map(|app| {
                                    let install_location = app
                                        .install_location
                                        .as_ref()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_default();
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &app.display_name,
                                            app.source,
                                            &app.uninstall_key,
                                            &install_location,
                                        ],
                                    )
                                    .then(|| InstalledAppExportRow {
                                        display_name: app.display_name.clone(),
                                        source: app.source.to_string(),
                                        install_location: (!install_location.trim().is_empty())
                                            .then_some(install_location),
                                        uninstall_key: app.uninstall_key.clone(),
                                    })
                                })
                                .collect();
                            let filtered_installed_backup_count = preview
                                .installed_apps
                                .iter()
                                .filter(|app| {
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &app.display_name,
                                            app.source,
                                            &app.uninstall_key,
                                            &app
                                                .install_location
                                                .as_ref()
                                                .map(|path| path.display().to_string())
                                                .unwrap_or_default(),
                                        ],
                                    ) && self
                                        .selected_installed_app_dirs
                                        .contains(&app.selection_key())
                                })
                                .count();
                            let filtered_installed_backup_available_count = preview
                                .installed_apps
                                .iter()
                                .filter(|app| {
                                    matches_filter(
                                        &self.scan_filter,
                                        &[
                                            &app.display_name,
                                            app.source,
                                            &app.uninstall_key,
                                            &app
                                                .install_location
                                                .as_ref()
                                                .map(|path| path.display().to_string())
                                                .unwrap_or_default(),
                                        ],
                                    ) && app.can_backup_files()
                                })
                                .count();

                            let render_scan_personal_files = |ui: &mut egui::Ui,
                                                              selected_user_roots: &mut HashSet<String>,
                                                              scan_filter: &mut String,
                                                              scan_user_root_summary: &ScanUserRootSummary,
                                                              last_notice: &mut Option<String>,
                                                              last_error: &mut Option<String>| {
                                card_panel(
                                    ui,
                                    "",
                                    "",
                                    |ui| {
                                        search_toolbar(ui, scan_filter, "输入名称或路径搜索个人文件");
                                        ui.add_space(8.0);
                                        ui.horizontal_wrapped(|ui| {
                                            section_counter(
                                                ui,
                                                "筛选命中",
                                                scan_user_root_summary.filtered_count,
                                            );
                                            section_counter(
                                                ui,
                                                "已选中",
                                                scan_user_root_summary.visible_selected_count,
                                            );
                                            section_counter(
                                                ui,
                                                "未选中",
                                                scan_user_root_summary.visible_unselected_count,
                                            );
                                            if ui.add(secondary_action_button("全选命中")).clicked() {
                                                for key in &scan_user_root_summary.visible_keys {
                                                    selected_user_roots.insert(key.clone());
                                                }
                                            }
                                            if ui.add(secondary_action_button("反选命中")).clicked() {
                                                for key in &scan_user_root_summary.visible_keys {
                                                    if !selected_user_roots.remove(key) {
                                                        selected_user_roots.insert(key.clone());
                                                    }
                                                }
                                            }
                                            if ui.add(secondary_action_button("清空命中")).clicked() {
                                                for key in &scan_user_root_summary.visible_keys {
                                                    selected_user_roots.remove(key);
                                                }
                                            }
                                        });
                                        ui.add_space(8.0);
                                        if scan_user_root_summary.filtered_count == 0 {
                                            compact_empty_state(
                                                ui,
                                                "没有命中的用户目录",
                                                "当前筛选词没有匹配到目录或配置项。",
                                            );
                                        } else {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(ui.available_width(), 620.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| {
                                                    egui::ScrollArea::vertical()
                                                        .auto_shrink([false, false])
                                                        .show(ui, |ui| {
                                                            for root in &preview.user_data_roots {
                                                                let path =
                                                                    root.path.display().to_string();
                                                                if !matches_scan_user_root(
                                                                    root,
                                                                    scan_filter.as_str(),
                                                                ) {
                                                                    continue;
                                                                }
                                                                let key =
                                                                    plan::path_key(&root.path);
                                                                let mut selected =
                                                                    selected_user_roots
                                                                        .contains(&key);
                                                                if selection_toggle_with_badge(
                                                                    ui,
                                                                    &mut selected,
                                                                    path_kind_badge_from_path(
                                                                        Some(&root.path),
                                                                    ),
                                                                    &root.label,
                                                                )
                                                                .changed()
                                                                {
                                                                    if selected {
                                                                        selected_user_roots.insert(
                                                                            key.clone(),
                                                                        );
                                                                    } else {
                                                                        selected_user_roots
                                                                            .remove(&key);
                                                                    }
                                                                }
                                                                selection_result_card(
                                                                    ui,
                                                                    selected,
                                                                    "",
                                                                    "",
                                                                    |ui| {
                                                                        detail_line(
                                                                            ui,
                                                                            format!(
                                                                                "路径：{}",
                                                                                path
                                                                            ),
                                                                        );
                                                                        detail_line(
                                                                            ui,
                                                                            format!(
                                                                                "预计大小：{}，共 {} 个文件",
                                                                                format_bytes(
                                                                                    root.stats.total_bytes
                                                                                ),
                                                                                root.stats.file_count
                                                                            ),
                                                                        );
                                                                        ui.add_space(4.0);
                                                                        if ui
                                                                            .add(
                                                                                secondary_action_button(
                                                                                    "打开所在路径",
                                                                                ),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            if let Err(error) =
                                                                                open_containing_path_in_explorer(
                                                                                    &root.path,
                                                                                )
                                                                            {
                                                                                *last_error = Some(
                                                                                    error.to_string(),
                                                                                );
                                                                                *last_notice = None;
                                                                            }
                                                                        }
                                                                    },
                                                                );
                                                                ui.add_space(6.0);
                                                            }
                                                        });
                                                },
                                            );
                                        }
                                    },
                                );
                            };

                            let render_scan_portable_apps = |ui: &mut egui::Ui,
                                                             selected_portable_apps: &mut HashSet<String>,
                                                             scan_filter: &mut String,
                                                             filtered_portable_count: usize,
                                                             visible_portable_keys: &[String],
                                                             last_notice: &mut Option<String>,
                                                             last_error: &mut Option<String>| {
                                card_panel(
                                    ui,
                                    "",
                                    "",
                                    |ui| {
                                        search_toolbar(ui, scan_filter, "输入名称或路径搜索便携软件");
                                        ui.add_space(8.0);
                                        ui.horizontal_wrapped(|ui| {
                                            section_counter(
                                                ui,
                                                "筛选命中",
                                                filtered_portable_count,
                                            );
                                            if ui.add(secondary_action_button("全选命中")).clicked() {
                                                for key in visible_portable_keys {
                                                    selected_portable_apps.insert(key.clone());
                                                }
                                            }
                                            if ui.add(secondary_action_button("反选命中")).clicked() {
                                                for key in visible_portable_keys {
                                                    if !selected_portable_apps.remove(key) {
                                                        selected_portable_apps.insert(key.clone());
                                                    }
                                                }
                                            }
                                            if ui.add(secondary_action_button("清空命中")).clicked() {
                                                for key in visible_portable_keys {
                                                    selected_portable_apps.remove(key);
                                                }
                                            }
                                        });
                                        ui.add_space(8.0);
                                        if filtered_portable_count == 0 {
                                            compact_empty_state(
                                                ui,
                                                "没有命中的便携候选",
                                                "当前筛选词没有匹配到可打包的便携程序。",
                                            );
                                        } else {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(ui.available_width(), 620.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| {
                                                    egui::ScrollArea::vertical()
                                                        .auto_shrink([false, false])
                                                        .show(ui, |ui| {
                                                            for item in preview
                                                                .portable_candidates
                                                                .iter()
                                                                .take(60)
                                                            {
                                                                let root_path = item
                                                                    .root_path
                                                                    .display()
                                                                    .to_string();
                                                                let main_executable = item
                                                                    .main_executable
                                                                    .display()
                                                                    .to_string();
                                                                if !matches_filter(
                                                                    scan_filter.as_str(),
                                                                    &[
                                                                        &item.display_name,
                                                                        &root_path,
                                                                        &main_executable,
                                                                        item.confidence_label(),
                                                                    ],
                                                                ) {
                                                                    continue;
                                                                }
                                                                let key =
                                                                    plan::path_key(&item.root_path);
                                                                let mut selected =
                                                                    selected_portable_apps
                                                                        .contains(&key);
                                                                if selection_toggle_with_badge(
                                                                    ui,
                                                                    &mut selected,
                                                                    path_kind_badge_from_path(
                                                                        Some(&item.root_path),
                                                                    ),
                                                                    &item.display_name,
                                                                )
                                                                .changed()
                                                                {
                                                                    if selected {
                                                                        selected_portable_apps
                                                                            .insert(key.clone());
                                                                    } else {
                                                                        selected_portable_apps
                                                                            .remove(&key);
                                                                    }
                                                                }
                                                                selection_result_card(
                                                                    ui,
                                                                    selected,
                                                                    "",
                                                                    "",
                                                                    |ui| {
                                                                        detail_line(
                                                                            ui,
                                                                            format!(
                                                                                "来源路径：{}",
                                                                                root_path
                                                                            ),
                                                                        );
                                                                        detail_line(
                                                                            ui,
                                                                            format!(
                                                                                "主程序：{}",
                                                                                main_executable
                                                                            ),
                                                                        );
                                                                        detail_line(
                                                                            ui,
                                                                            format!(
                                                                                "预计大小：{}",
                                                                                format_bytes(item.stats.total_bytes)
                                                                            ),
                                                                        );
                                                                        ui.add_space(4.0);
                                                                        if ui
                                                                            .add(
                                                                                secondary_action_button(
                                                                                    "打开所在路径",
                                                                                ),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            if let Err(error) =
                                                                                open_containing_path_in_explorer(
                                                                                    &item.root_path,
                                                                                )
                                                                            {
                                                                                *last_error = Some(
                                                                                    error.to_string(),
                                                                                );
                                                                                *last_notice = None;
                                                                            }
                                                                        }
                                                                    },
                                                                );
                                                                ui.add_space(6.0);
                                                            }
                                                        });
                                                },
                                            );
                                        }
                                    },
                                );
                            };

                            backup_workflow_switcher(
                                ui,
                                &mut self.backup_page,
                                !preview.user_data_roots.is_empty(),
                                !preview.portable_candidates.is_empty(),
                                !preview.installed_apps.is_empty(),
                                !preview.user_data_roots.is_empty()
                                    || !preview.portable_candidates.is_empty()
                                    || !preview.installed_apps.is_empty(),
                            );

                            match self.backup_page {
                                BackupWorkflowPage::ScanScope => render_scan_scope_page(ui, self),
                                BackupWorkflowPage::UserData => {
                                    render_scan_personal_files(
                                        ui,
                                        &mut self.selected_user_roots,
                                        &mut self.scan_filter,
                                        &scan_user_root_summary,
                                        &mut self.last_notice,
                                        &mut self.last_error,
                                    );
                                    let _ = self.persist_config();
                                }
                                BackupWorkflowPage::PortableApps => {
                                    render_scan_portable_apps(
                                        ui,
                                        &mut self.selected_portable_apps,
                                        &mut self.scan_filter,
                                        filtered_portable_count,
                                        &visible_portable_keys,
                                        &mut self.last_notice,
                                        &mut self.last_error,
                                    );
                                    let _ = self.persist_config();
                                }
                                BackupWorkflowPage::InstalledApps => {
                                    let installed_dirs_dirty = render_scan_installed_apps_panel(
                                        ui,
                                        &preview,
                                        &mut self.scan_filter,
                                        filtered_installed_count,
                                        filtered_installed_backup_available_count,
                                        filtered_installed_backup_count,
                                        &filtered_scan_apps,
                                        &mut self.selected_installed_app_dirs,
                                        self.backup_output_input.trim(),
                                        &mut self.last_notice,
                                        &mut self.last_error,
                                    );
                                    if installed_dirs_dirty {
                                        let _ = self.persist_config();
                                    }
                                }
                                BackupWorkflowPage::Output => {
                                    review_list_panel(
                                        ui,
                                        "",
                                        "",
                                        720.0,
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("备份输出目录");
                                                if ui
                                                    .add(
                                                        egui::TextEdit::singleline(
                                                            &mut self.backup_output_input,
                                                        )
                                                        .desired_width(360.0)
                                                        .hint_text(
                                                            "例如 D:\\WinRehome Backups",
                                                        ),
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                    self.refresh_recent_archives();
                                                }
                                                if ui
                                                    .add(secondary_action_button("浏览目录"))
                                                    .clicked()
                                                {
                                                    if let Some(path) = pick_folder_from_input(
                                                        &self.backup_output_input,
                                                    ) {
                                                        self.backup_output_input =
                                                            path.display().to_string();
                                                        let _ = self.persist_config();
                                                        self.refresh_recent_archives();
                                                    }
                                                }
                                                if ui
                                                    .add(secondary_action_button("默认目录"))
                                                    .clicked()
                                                {
                                                    if let Ok(path) =
                                                        archive::default_output_dir()
                                                    {
                                                        self.backup_output_input =
                                                            path.display().to_string();
                                                        let _ = self.persist_config();
                                                        self.refresh_recent_archives();
                                                    }
                                                }
                                            });
                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                metric_tile(
                                                    ui,
                                                    Color32::from_rgb(245, 248, 252),
                                                    "已选个人目录",
                                                    &summary.selected_user_roots.to_string(),
                                                );
                                                metric_tile(
                                                    ui,
                                                    Color32::from_rgb(245, 248, 252),
                                                    "已选便携软件",
                                                    &summary.selected_portable_apps.to_string(),
                                                );
                                                metric_tile(
                                                    ui,
                                                    Color32::from_rgb(245, 248, 252),
                                                    "安装软件目录",
                                                    &summary.selected_installed_app_dirs.to_string(),
                                                );
                                                metric_tile(
                                                    ui,
                                                    Color32::from_rgb(245, 248, 252),
                                                    "预计文件数",
                                                    &summary.total_files.to_string(),
                                                );
                                                metric_tile(
                                                    ui,
                                                    Color32::from_rgb(245, 248, 252),
                                                    "预计大小",
                                                    &format_bytes(summary.total_bytes),
                                                );
                                            });
                                            ui.add_space(8.0);
                                            if summary.total_files == 0 {
                                                compact_empty_state(
                                                    ui,
                                                    "还没有备份内容",
                                                    "先到个人文件页、便携软件页或安装软件页选择至少一个项目。",
                                                );
                                            } else if let Ok(preflight) = &backup_preflight {
                                                status_banner(
                                                    ui,
                                                    Color32::from_rgb(232, 239, 248),
                                                    Color32::from_rgb(130, 155, 186),
                                                    &format!(
                                                        "预检：将把 {} 个文件写入 {}。目标目录{}。",
                                                        summary.total_files,
                                                        preflight.output_dir.display(),
                                                        if preflight.exists
                                                            && preflight.is_directory
                                                        {
                                                            "已存在"
                                                        } else {
                                                            "将由 WinRehome 自动创建"
                                                        }
                                                    ),
                                                );
                                            } else if let Err(error) = &backup_preflight {
                                                status_banner(
                                                    ui,
                                                    Color32::from_rgb(252, 233, 229),
                                                    Color32::from_rgb(212, 122, 102),
                                                    &present_backup_error(&error.to_string()),
                                                );
                                            }
                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                if ui
                                                    .add_enabled(
                                                        summary.total_files > 0
                                                            && backup_preflight.is_ok(),
                                                        primary_action_button("创建备份归档"),
                                                    )
                                                    .clicked()
                                                {
                                                    if let Ok(preflight) = &backup_preflight {
                                                        let path = preflight.output_dir.clone();
                                                        let default_output_dir =
                                                            archive::default_output_dir().ok();
                                                        let backup_result =
                                                            if default_output_dir.as_ref().is_some_and(
                                                                |default_dir| default_dir == &path,
                                                            ) {
                                                                archive::create_backup_archive(
                                                                    &preview,
                                                                    &self.selected_user_roots,
                                                                    &self.selected_portable_apps,
                                                                    &self.selected_installed_app_dirs,
                                                                )
                                                            } else {
                                                                archive::create_backup_archive_in_dir(
                                                                    &preview,
                                                                    &self.selected_user_roots,
                                                                    &self.selected_portable_apps,
                                                                    &self.selected_installed_app_dirs,
                                                                    &path,
                                                                )
                                                            };

                                                        match backup_result {
                                                            Ok(result) => {
                                                                self.load_archive_from_path(
                                                                    result.archive_path.clone(),
                                                                );
                                                                self.last_archive = Some(result);
                                                                self.last_verification = None;
                                                                self.last_restore = None;
                                                                self.last_notice = None;
                                                                self.last_error = None;
                                                                let _ = self.persist_config();
                                                            }
                                                            Err(error) => {
                                                                self.last_archive = None;
                                                                self.last_error = Some(
                                                                    present_backup_error(
                                                                        &error.to_string(),
                                                                    ),
                                                                );
                                                                self.last_notice = None;
                                                            }
                                                        }
                                                    }
                                                }
                                            });
                                        },
                                    );
                                }
                            }
                        } else {
                            backup_workflow_switcher(
                                ui,
                                &mut self.backup_page,
                                false,
                                false,
                                false,
                                false,
                            );

                            match self.backup_page {
                                BackupWorkflowPage::ScanScope => render_scan_scope_page(ui, self),
                                BackupWorkflowPage::UserData => {
                                    review_list_panel(
                                        ui,
                                        "",
                                        "",
                                        720.0,
                                        |ui| {
                                            waiting_scan_result_state(ui);
                                        },
                                    );
                                }
                                BackupWorkflowPage::PortableApps => {
                                    review_list_panel(
                                        ui,
                                        "",
                                        "",
                                        720.0,
                                        |ui| {
                                            waiting_scan_result_state(ui);
                                        },
                                    );
                                }
                                BackupWorkflowPage::InstalledApps => {
                                    review_list_panel(
                                        ui,
                                        "",
                                        "",
                                        720.0,
                                        |ui| {
                                            waiting_scan_result_state(ui);
                                        },
                                    );
                                }
                                BackupWorkflowPage::Output => {
                                    review_list_panel(
                                        ui,
                                        "",
                                        "",
                                        720.0,
                                        |ui| {
                                            waiting_scan_result_state(ui);
                                        },
                                    );
                                }
                            }
                        }
                    }

                        if matches!(resolved_workspace, WorkspaceView::Restore) {
                        if let Some(loaded) = self.loaded_archive.clone() {
                            let filtered_restore_user_count = loaded
                                .manifest
                                .selected_user_roots
                                .iter()
                                .filter(|root| {
                                    matches_filter(
                                        &self.restore_filter,
                                        &[&root.label, &root.category, &root.path],
                                    )
                                })
                                .count();
                            let filtered_restore_portable_count = loaded
                                .manifest
                                .selected_portable_apps
                                .iter()
                                .filter(|app| {
                                    matches_filter(
                                        &self.restore_filter,
                                        &[&app.display_name, &app.root_path, &app.main_executable],
                                    )
                                })
                                .count();
                            let filtered_restore_installed_count = loaded
                                .manifest
                                .installed_apps
                                .iter()
                                .filter(|app| {
                                    matches_filter(
                                        &self.restore_inventory_filter,
                                        &[
                                            &app.display_name,
                                            &app.source,
                                            &app.uninstall_key,
                                            &app.install_location.clone().unwrap_or_default(),
                                        ],
                                    )
                                })
                                .count();
                            let filtered_restore_installed_dir_count = loaded
                                .manifest
                                .installed_apps
                                .iter()
                                .filter(|app| {
                                    app.backup_root.is_some()
                                        && matches_filter(
                                            &self.restore_filter,
                                            &[
                                                &app.display_name,
                                                &app.source,
                                                &app.install_location.clone().unwrap_or_default(),
                                            ],
                                        )
                                })
                                .count();
                            let restore_scope_user_summary = summarize_restore_user_roots(
                                &loaded.manifest.selected_user_roots,
                                &self.restore_filter,
                                self.restore_scope_only_unselected_roots,
                                &self.selected_restore_roots,
                            );
                            let restore_scope_portable_summary =
                                summarize_restore_portable_apps(
                                    &loaded.manifest.selected_portable_apps,
                                    &self.restore_filter,
                                    self.restore_scope_only_unselected_roots,
                                    &self.selected_restore_roots,
                                );
                            let restore_scope_installed_summary =
                                summarize_restore_installed_app_dirs(
                                    &loaded.manifest.installed_apps,
                                    &self.restore_filter,
                                    self.restore_scope_only_unselected_roots,
                                    &self.selected_restore_roots,
                                );
                            let all_restore_roots = collect_restore_roots(&loaded);
                        let effective_restore_roots = effective_restore_roots(
                            &loaded,
                            self.restore_user_data,
                            self.restore_portable_apps,
                            self.restore_installed_app_dirs,
                            &self.selected_restore_roots,
                        );
                        let restore_summary =
                            build_restore_preview_summary(&loaded, &effective_restore_roots);
                        let restore_preflight = if self.restore_destination_input.trim().is_empty()
                            || restore_summary.selected_file_count == 0
                        {
                            None
                        } else {
                            Some(archive::preview_restore_with_manifest(
                                Path::new(self.restore_destination_input.trim()),
                                &loaded.manifest,
                                &archive::RestoreSelection {
                                    restore_user_data: self.restore_user_data,
                                    restore_portable_apps: self.restore_portable_apps,
                                    restore_installed_app_dirs: self.restore_installed_app_dirs,
                                    selected_roots: effective_restore_roots.clone(),
                                    skip_existing_files: self.skip_existing_restore_files,
                                },
                            ))
                        };

                            restore_workflow_switcher(
                                ui,
                                true,
                                Some(&mut self.restore_section),
                            );
                            ui.add_space(8.0);
                            card_panel(
                                ui,
                                "恢复备份",
                                "",
                                |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "归档大小",
                                            &format_bytes(loaded.manifest.stored_bytes),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "已安装软件记录",
                                            &loaded.manifest.installed_apps.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "安装软件目录",
                                            &loaded
                                                .manifest
                                                .installed_apps
                                                .iter()
                                                .filter(|app| app.backup_root.is_some())
                                                .count()
                                                .to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "个人文件目录",
                                            &loaded.manifest.selected_user_roots.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "便携软件",
                                            &loaded
                                                .manifest
                                                .selected_portable_apps
                                                .len()
                                                .to_string(),
                                        );
                                    });

                                    ui.add_space(10.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("步骤 1：归档文件");
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(
                                                    &mut self.archive_path_input,
                                                )
                                                .desired_width(360.0)
                                                .hint_text("选择或输入 .wrh 归档路径"),
                                            )
                                            .changed()
                                        {
                                            let _ = self.persist_config();
                                        }
                                        if ui.add(secondary_action_button("浏览归档")).clicked() {
                                            if let Some(path) =
                                                pick_archive_file_from_input(&self.archive_path_input)
                                            {
                                                self.load_archive_from_path(path);
                                            }
                                        }
                                        if ui.add(secondary_action_button("加载归档")).clicked() {
                                            let path =
                                                PathBuf::from(self.archive_path_input.trim());
                                            self.load_archive_from_path(path);
                                        }
                                        if ui.add(secondary_action_button("定位归档")).clicked() {
                                            let path = PathBuf::from(self.archive_path_input.trim());
                                            if self.archive_path_input.trim().is_empty() {
                                                self.last_error =
                                                    Some("请先选择一个归档文件。".to_string());
                                                self.last_notice = None;
                                            } else if let Err(error) = open_path_in_explorer(&path)
                                            {
                                                self.last_error = Some(error.to_string());
                                                self.last_notice = None;
                                            }
                                        }
                                    });

                                    let archive_name = loaded
                                        .path
                                        .file_name()
                                        .and_then(|value| value.to_str())
                                        .unwrap_or("未知归档");
                                    ui.small(format!(
                                        "归档名称：{} | 当前已选恢复范围：{} / {}",
                                        archive_name,
                                        self.selected_restore_roots.len(),
                                        all_restore_roots.len()
                                    ));

                                    ui.add_space(8.0);
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "已选个人目录",
                                            &restore_summary.selected_user_root_count.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "已选便携软件",
                                            &restore_summary
                                                .selected_portable_app_count
                                                .to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "已选安装目录",
                                            &restore_summary
                                                .selected_installed_app_dir_count
                                                .to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "预计恢复文件",
                                            &restore_summary.selected_file_count.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "预计恢复大小",
                                            &format_bytes(restore_summary.selected_bytes),
                                        );
                                    });
                                },
                            );

                            match self.restore_section {
                                RestoreSection::InstalledApps => {
                                    let filtered_restore_apps: Vec<InstalledAppExportRow> = loaded
                                        .manifest
                                        .installed_apps
                                        .iter()
                                        .filter(|app| {
                                            matches_filter(
                                                &self.restore_inventory_filter,
                                                &[
                                                    &app.display_name,
                                                    &app.source,
                                                    &app.uninstall_key,
                                                    &app.install_location.clone().unwrap_or_default(),
                                                ],
                                            )
                                        })
                                        .map(|app| InstalledAppExportRow {
                                            display_name: app.display_name.clone(),
                                            source: app.source.clone(),
                                            install_location: app.install_location.clone(),
                                            uninstall_key: app.uninstall_key.clone(),
                                        })
                                        .collect();
                                    card_panel(
                                        ui,
                                        "步骤 2：查看备份内容",
                                        "",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("内容筛选");
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.restore_filter,
                                                    )
                                                    .desired_width(280.0)
                                                    .hint_text("按名称或路径过滤个人文件与便携软件"),
                                                );
                                                ui.label("软件记录筛选");
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.restore_inventory_filter,
                                                    )
                                                    .desired_width(240.0)
                                                    .hint_text("按软件名、来源或安装路径过滤"),
                                                );
                                            });
                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(
                                                    ui,
                                                    "个人文件",
                                                    filtered_restore_user_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "便携软件",
                                                    filtered_restore_portable_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "安装目录",
                                                    filtered_restore_installed_dir_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "软件记录",
                                                    filtered_restore_installed_count,
                                                );
                                                if ui
                                                    .add(secondary_action_button("导出命中 CSV"))
                                                    .clicked()
                                                {
                                                    let archive_stem = loaded
                                                        .path
                                                        .file_stem()
                                                        .and_then(|value| value.to_str())
                                                        .unwrap_or("loaded-archive");
                                                    let default_name = format!(
                                                        "{archive_stem}-installed-apps.csv"
                                                    );
                                                    match pick_inventory_export_path(
                                                        &default_name,
                                                        loaded.path.parent().and_then(|path| {
                                                            path.to_str()
                                                        }),
                                                    ) {
                                                        Some(path) => {
                                                            match export_installed_app_inventory_csv(
                                                                &path,
                                                                &filtered_restore_apps,
                                                            ) {
                                                                Ok(count) => {
                                                                    self.last_notice = Some(format!(
                                                                        "归档内的软件记录已导出：{}，共 {} 条。",
                                                                        path.display(),
                                                                        count
                                                                    ));
                                                                    self.last_error = None;
                                                                }
                                                                Err(error) => {
                                                                    self.last_error = Some(
                                                                        error.to_string(),
                                                                    );
                                                                    self.last_notice = None;
                                                                }
                                                            }
                                                        }
                                                        None if filtered_restore_apps.is_empty() => {
                                                            self.last_error = Some(
                                                                "当前没有可导出的归档软件记录。"
                                                                    .to_string(),
                                                            );
                                                            self.last_notice = None;
                                                        }
                                                        None => {}
                                                    }
                                                }
                                            });
                                            ui.add_space(8.0);
                                            let show_restore_preview_panel =
                                                |ui: &mut egui::Ui,
                                                 title: &str,
                                                 meta: &str,
                                                 empty_title: &str,
                                                 empty_body: &str,
                                                 has_items: bool,
                                                 add_items: &mut dyn FnMut(&mut egui::Ui)| {
                                                    review_list_panel(ui, title, meta, 340.0, |ui| {
                                                        if !has_items {
                                                            compact_empty_state(
                                                                ui,
                                                                empty_title,
                                                                empty_body,
                                                            );
                                                        } else {
                                                            add_items(ui);
                                                        }
                                                    });
                                                };

                                            if ui.available_width() > 920.0 {
                                                ui.columns(3, |columns| {
                                                    let mut user_items = |ui: &mut egui::Ui| {
                                                        for root in &loaded.manifest.selected_user_roots
                                                        {
                                                            if !matches_filter(
                                                                &self.restore_filter,
                                                                &[
                                                                    &root.label,
                                                                    &root.category,
                                                                    &root.path,
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            result_card(
                                                                ui,
                                                                &root.label,
                                                                &root.category,
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "路径：{}",
                                                                        root.path
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    };
                                                    show_restore_preview_panel(
                                                        &mut columns[0],
                                                        "个人文件",
                                                        "这些目录和文件会参与恢复范围选择。",
                                                        "没有命中的个人文件",
                                                        "调整筛选词后，这里会显示归档里的个人文件目录。",
                                                        filtered_restore_user_count > 0,
                                                        &mut user_items,
                                                    );

                                                    let mut portable_items = |ui: &mut egui::Ui| {
                                                        for app in &loaded.manifest.selected_portable_apps
                                                        {
                                                            if !matches_filter(
                                                                &self.restore_filter,
                                                                &[
                                                                    &app.display_name,
                                                                    &app.root_path,
                                                                    &app.main_executable,
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            result_card(
                                                                ui,
                                                                &app.display_name,
                                                                "便携软件",
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "路径：{}",
                                                                        app.root_path
                                                                    ));
                                                                    ui.small(format!(
                                                                        "主程序：{}",
                                                                        app.main_executable
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    };
                                                    show_restore_preview_panel(
                                                        &mut columns[1],
                                                        "便携软件",
                                                        "这些项目可随归档一起恢复。",
                                                        "没有命中的便携软件",
                                                        "当前筛选词下，没有命中的便携软件。",
                                                        filtered_restore_portable_count > 0,
                                                        &mut portable_items,
                                                    );

                                                    let mut installed_items = |ui: &mut egui::Ui| {
                                                        status_banner(
                                                            ui,
                                                            Color32::from_rgb(235, 242, 251),
                                                            Color32::from_rgb(179, 201, 231),
                                                            if loaded
                                                                .manifest
                                                                .installed_apps
                                                                .iter()
                                                                .any(|app| app.backup_root.is_some())
                                                            {
                                                                "已安装软件仍然需要重新安装；带“安装目录已备份”的项目，可额外恢复其目录文件。"
                                                            } else {
                                                                "这些软件需要在新系统中手动重新安装，WinRehome 只保留清单记录。"
                                                            },
                                                        );
                                                        ui.add_space(8.0);
                                                        for app in loaded
                                                            .manifest
                                                            .installed_apps
                                                            .iter()
                                                            .take(160)
                                                        {
                                                            if !matches_filter(
                                                                &self.restore_inventory_filter,
                                                                &[
                                                                    &app.display_name,
                                                                    &app.source,
                                                                    &app.uninstall_key,
                                                                    &app
                                                                        .install_location
                                                                        .clone()
                                                                        .unwrap_or_default(),
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            result_card(
                                                                ui,
                                                                &app.display_name,
                                                                &format!("来源：{}", &app.source),
                                                                |ui| {
                                                                    if app.files_included {
                                                                        ui.small(
                                                                            "安装目录已备份，可在恢复范围中单独选择。",
                                                                        );
                                                                    }
                                                                    if let Some(path) =
                                                                        &app.install_location
                                                                    {
                                                                        ui.small(format!(
                                                                            "安装位置：{}",
                                                                            path
                                                                        ));
                                                                    }
                                                                    ui.small(format!(
                                                                        "注册表键：{}",
                                                                        app.uninstall_key
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    };
                                                    show_restore_preview_panel(
                                                        &mut columns[2],
                                                        "已安装软件记录",
                                                        "这里展示软件记录；若归档包含安装目录，也会注明。",
                                                        "没有命中的软件记录",
                                                        "调整筛选词后，可以在这里查看安装版软件清单。",
                                                        filtered_restore_installed_count > 0,
                                                        &mut installed_items,
                                                    );
                                                });
                                            } else {
                                                let mut user_items = |ui: &mut egui::Ui| {
                                                    for root in &loaded.manifest.selected_user_roots {
                                                        if !matches_filter(
                                                            &self.restore_filter,
                                                            &[
                                                                &root.label,
                                                                &root.category,
                                                                &root.path,
                                                            ],
                                                        ) {
                                                            continue;
                                                        }
                                                        result_card(
                                                            ui,
                                                            &root.label,
                                                            &root.category,
                                                            |ui| {
                                                                ui.small(format!(
                                                                    "路径：{}",
                                                                    root.path
                                                                ));
                                                            },
                                                        );
                                                        ui.add_space(6.0);
                                                    }
                                                };
                                                show_restore_preview_panel(
                                                    ui,
                                                    "个人文件",
                                                    "这些目录和文件会参与恢复范围选择。",
                                                    "没有命中的个人文件",
                                                    "调整筛选词后，这里会显示归档里的个人文件目录。",
                                                    filtered_restore_user_count > 0,
                                                    &mut user_items,
                                                );
                                                ui.add_space(10.0);

                                                let mut portable_items = |ui: &mut egui::Ui| {
                                                    for app in &loaded.manifest.selected_portable_apps {
                                                        if !matches_filter(
                                                            &self.restore_filter,
                                                            &[
                                                                &app.display_name,
                                                                &app.root_path,
                                                                &app.main_executable,
                                                            ],
                                                        ) {
                                                            continue;
                                                        }
                                                        result_card(
                                                            ui,
                                                            &app.display_name,
                                                            "便携软件",
                                                            |ui| {
                                                                ui.small(format!(
                                                                    "路径：{}",
                                                                    app.root_path
                                                                ));
                                                                ui.small(format!(
                                                                    "主程序：{}",
                                                                    app.main_executable
                                                                ));
                                                            },
                                                        );
                                                        ui.add_space(6.0);
                                                    }
                                                };
                                                show_restore_preview_panel(
                                                    ui,
                                                    "便携软件",
                                                    "这些项目可随归档一起恢复。",
                                                    "没有命中的便携软件",
                                                    "当前筛选词下，没有命中的便携软件。",
                                                    filtered_restore_portable_count > 0,
                                                    &mut portable_items,
                                                );
                                                ui.add_space(10.0);

                                                let mut installed_items = |ui: &mut egui::Ui| {
                                                    status_banner(
                                                        ui,
                                                        Color32::from_rgb(235, 242, 251),
                                                        Color32::from_rgb(179, 201, 231),
                                                        if loaded
                                                            .manifest
                                                            .installed_apps
                                                            .iter()
                                                            .any(|app| app.backup_root.is_some())
                                                        {
                                                            "已安装软件仍然需要重新安装；带“安装目录已备份”的项目，可额外恢复其目录文件。"
                                                        } else {
                                                            "这些软件需要在新系统中手动重新安装，WinRehome 只保留清单记录。"
                                                        },
                                                    );
                                                    ui.add_space(8.0);
                                                    for app in loaded
                                                        .manifest
                                                        .installed_apps
                                                        .iter()
                                                        .take(160)
                                                    {
                                                        if !matches_filter(
                                                            &self.restore_inventory_filter,
                                                            &[
                                                                &app.display_name,
                                                                &app.source,
                                                                &app.uninstall_key,
                                                                &app
                                                                    .install_location
                                                                    .clone()
                                                                    .unwrap_or_default(),
                                                            ],
                                                        ) {
                                                            continue;
                                                        }
                                                        result_card(
                                                            ui,
                                                            &app.display_name,
                                                            &format!("来源：{}", &app.source),
                                                            |ui| {
                                                                if app.files_included {
                                                                    ui.small(
                                                                        "安装目录已备份，可在恢复范围中单独选择。",
                                                                    );
                                                                }
                                                                if let Some(path) =
                                                                    &app.install_location
                                                                {
                                                                    ui.small(format!(
                                                                        "安装位置：{}",
                                                                        path
                                                                    ));
                                                                }
                                                                ui.small(format!(
                                                                    "注册表键：{}",
                                                                    app.uninstall_key
                                                                ));
                                                            },
                                                        );
                                                        ui.add_space(6.0);
                                                    }
                                                };
                                                show_restore_preview_panel(
                                                    ui,
                                                    "已安装软件记录",
                                                    "这里展示软件记录；若归档包含安装目录，也会注明。",
                                                    "没有命中的软件记录",
                                                    "调整筛选词后，可以在这里查看安装版软件清单。",
                                                    filtered_restore_installed_count > 0,
                                                    &mut installed_items,
                                                );
                                            }
                                        },
                                    );
                                }
                                RestoreSection::RestoreScope => {
                                    card_panel(
                                        ui,
                                        "步骤 3：选择恢复范围",
                                        "",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if ui
                                                    .checkbox(
                                                        &mut self.restore_user_data,
                                                        "恢复个人文件",
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                }
                                                if ui
                                                    .checkbox(
                                                        &mut self.restore_portable_apps,
                                                        "恢复便携软件",
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                }
                                                if ui
                                                    .checkbox(
                                                        &mut self.restore_installed_app_dirs,
                                                        "恢复安装目录",
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                }
                                            });

                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("范围筛选");
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.restore_filter,
                                                    )
                                                    .desired_width(320.0)
                                                    .hint_text("按名称或路径过滤恢复范围"),
                                                );
                                                ui.checkbox(
                                                    &mut self.restore_scope_only_unselected_roots,
                                                    "只看未选中项",
                                                );
                                                if ui.add(secondary_action_button("全部选择")).clicked()
                                                {
                                                    self.selected_restore_roots =
                                                        all_restore_roots.clone();
                                                    let _ = self.persist_config();
                                                }
                                                if ui.add(secondary_action_button("全部清空")).clicked()
                                                {
                                                    self.selected_restore_roots.clear();
                                                    let _ = self.persist_config();
                                                }
                                            });

                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(
                                                    ui,
                                                    "当前已选范围",
                                                    restore_summary.selected_root_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "个人文件",
                                                    restore_summary.selected_user_root_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "便携软件",
                                                    restore_summary.selected_portable_app_count,
                                                );
                                                section_counter(
                                                    ui,
                                                    "安装目录",
                                                    restore_summary.selected_installed_app_dir_count,
                                                );
                                            });

                                            if !self.restore_filter.trim().is_empty()
                                                && restore_scope_user_summary.filtered_count == 0
                                                && restore_scope_portable_summary.filtered_count == 0
                                                && restore_scope_installed_summary.filtered_count == 0
                                            {
                                                ui.add_space(8.0);
                                                compact_empty_state(
                                                    ui,
                                                    "没有命中的恢复范围",
                                                    "当前筛选词没有匹配到任何用户目录、便携软件或安装目录。",
                                                );
                                            }

                                            if !loaded.manifest.selected_user_roots.is_empty() {
                                                ui.add_space(10.0);
                                                ui.label(RichText::new("个人文件目录").strong());
                                                ui.horizontal_wrapped(|ui| {
                                                    section_counter(
                                                        ui,
                                                        "筛选命中",
                                                        restore_scope_user_summary.filtered_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "已选中",
                                                        restore_scope_user_summary
                                                            .visible_selected_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "未选中",
                                                        restore_scope_user_summary
                                                            .visible_unselected_count,
                                                    );
                                                    if ui.add(secondary_action_button("全选命中")).clicked()
                                                    {
                                                        for key in &restore_scope_user_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots
                                                                .insert(key.clone());
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                    if ui.add(secondary_action_button("清空命中")).clicked()
                                                    {
                                                        for key in &restore_scope_user_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots.remove(key);
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                });
                                                ui.add_space(6.0);
                                                egui::ScrollArea::vertical()
                                                    .max_height(220.0)
                                                    .show(ui, |ui| {
                                                        for root in
                                                            &loaded.manifest.selected_user_roots
                                                        {
                                                            if !matches_restore_user_root(
                                                                root,
                                                                &self.restore_filter,
                                                                self.restore_scope_only_unselected_roots,
                                                                &self.selected_restore_roots,
                                                            ) {
                                                                continue;
                                                            }
                                                            let key =
                                                                user_restore_root_key(root);
                                                            let mut selected = self
                                                                .selected_restore_roots
                                                                .contains(&key);
                                                            if ui
                                                                .checkbox(
                                                                    &mut selected,
                                                                    &root.label,
                                                                )
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    self.selected_restore_roots
                                                                        .insert(key.clone());
                                                                } else {
                                                                    self.selected_restore_roots
                                                                        .remove(&key);
                                                                }
                                                                let _ = self.persist_config();
                                                            }
                                                            selection_result_card(
                                                                ui,
                                                                selected,
                                                                &root.label,
                                                                &root.category,
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "路径：{}",
                                                                        root.path
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }

                                            if !loaded.manifest.selected_portable_apps.is_empty() {
                                                ui.add_space(10.0);
                                                ui.label(RichText::new("便携软件").strong());
                                                ui.horizontal_wrapped(|ui| {
                                                    section_counter(
                                                        ui,
                                                        "筛选命中",
                                                        restore_scope_portable_summary
                                                            .filtered_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "已选中",
                                                        restore_scope_portable_summary
                                                            .visible_selected_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "未选中",
                                                        restore_scope_portable_summary
                                                            .visible_unselected_count,
                                                    );
                                                    if ui.add(secondary_action_button("全选命中")).clicked()
                                                    {
                                                        for key in &restore_scope_portable_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots
                                                                .insert(key.clone());
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                    if ui.add(secondary_action_button("清空命中")).clicked()
                                                    {
                                                        for key in &restore_scope_portable_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots.remove(key);
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                });
                                                ui.add_space(6.0);
                                                egui::ScrollArea::vertical()
                                                    .max_height(220.0)
                                                    .show(ui, |ui| {
                                                        for app in
                                                            &loaded.manifest.selected_portable_apps
                                                        {
                                                            if !matches_restore_portable_app(
                                                                app,
                                                                &self.restore_filter,
                                                                self.restore_scope_only_unselected_roots,
                                                                &self.selected_restore_roots,
                                                            ) {
                                                                continue;
                                                            }
                                                            let key =
                                                                portable_restore_root_key(app);
                                                            let mut selected = self
                                                                .selected_restore_roots
                                                                .contains(&key);
                                                            if ui
                                                                .checkbox(
                                                                    &mut selected,
                                                                    &app.display_name,
                                                                )
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    self.selected_restore_roots
                                                                        .insert(key.clone());
                                                                } else {
                                                                    self.selected_restore_roots
                                                                        .remove(&key);
                                                                }
                                                                let _ = self.persist_config();
                                                            }
                                                            selection_result_card(
                                                                ui,
                                                                selected,
                                                                &app.display_name,
                                                                "便携软件",
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "路径：{}",
                                                                        app.root_path
                                                                    ));
                                                                    ui.small(format!(
                                                                        "主程序：{}",
                                                                        app.main_executable
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }

                                            if loaded
                                                .manifest
                                                .installed_apps
                                                .iter()
                                                .any(|app| app.backup_root.is_some())
                                            {
                                                ui.add_space(10.0);
                                                ui.label(RichText::new("安装软件目录").strong());
                                                ui.horizontal_wrapped(|ui| {
                                                    section_counter(
                                                        ui,
                                                        "筛选命中",
                                                        restore_scope_installed_summary
                                                            .filtered_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "已选中",
                                                        restore_scope_installed_summary
                                                            .visible_selected_count,
                                                    );
                                                    section_counter(
                                                        ui,
                                                        "未选中",
                                                        restore_scope_installed_summary
                                                            .visible_unselected_count,
                                                    );
                                                    if ui.add(secondary_action_button("全选命中")).clicked()
                                                    {
                                                        for key in &restore_scope_installed_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots
                                                                .insert(key.clone());
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                    if ui.add(secondary_action_button("清空命中")).clicked()
                                                    {
                                                        for key in &restore_scope_installed_summary
                                                            .visible_keys
                                                        {
                                                            self.selected_restore_roots.remove(key);
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                });
                                                ui.add_space(6.0);
                                                egui::ScrollArea::vertical()
                                                    .max_height(220.0)
                                                    .show(ui, |ui| {
                                                        for app in &loaded.manifest.installed_apps {
                                                            if !matches_restore_installed_app(
                                                                app,
                                                                &self.restore_filter,
                                                                self.restore_scope_only_unselected_roots,
                                                                &self.selected_restore_roots,
                                                            ) {
                                                                continue;
                                                            }
                                                            let Some(key) =
                                                                installed_app_restore_root_key(app)
                                                            else {
                                                                continue;
                                                            };
                                                            let mut selected = self
                                                                .selected_restore_roots
                                                                .contains(&key);
                                                            if ui
                                                                .checkbox(
                                                                    &mut selected,
                                                                    &app.display_name,
                                                                )
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    self.selected_restore_roots
                                                                        .insert(key.clone());
                                                                } else {
                                                                    self.selected_restore_roots
                                                                        .remove(&key);
                                                                }
                                                                let _ = self.persist_config();
                                                            }
                                                            selection_result_card(
                                                                ui,
                                                                selected,
                                                                &app.display_name,
                                                                "安装软件目录",
                                                                |ui| {
                                                                    if let Some(path) =
                                                                        &app.install_location
                                                                    {
                                                                        ui.small(format!(
                                                                            "原安装位置：{}",
                                                                            path
                                                                        ));
                                                                    }
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }
                                        },
                                    );
                                }
                                RestoreSection::RestoreAction => {
                                    card_panel(
                                        ui,
                                        "步骤 3：执行恢复",
                                        "",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("恢复到");
                                                if ui
                                                    .add(
                                                        egui::TextEdit::singleline(
                                                            &mut self.restore_destination_input,
                                                        )
                                                        .desired_width(360.0)
                                                        .hint_text("例如 D:\\WinRehome Restore"),
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                }
                                                if ui.add(secondary_action_button("浏览目录")).clicked()
                                                {
                                                    if let Some(path) =
                                                        pick_folder_from_input(
                                                            &self.restore_destination_input,
                                                        )
                                                    {
                                                        self.restore_destination_input =
                                                            path.display().to_string();
                                                        let _ = self.persist_config();
                                                    }
                                                }
                                                if ui.add(secondary_action_button("默认目录")).clicked()
                                                {
                                                    if let Ok(path) =
                                                        archive::default_restore_dir(&loaded.path)
                                                    {
                                                        self.restore_destination_input =
                                                            path.display().to_string();
                                                        let _ = self.persist_config();
                                                    }
                                                }
                                                if ui.add(secondary_action_button("打开目录")).clicked()
                                                {
                                                    let path = PathBuf::from(
                                                        self.restore_destination_input.trim(),
                                                    );
                                                    if self.restore_destination_input.trim().is_empty()
                                                    {
                                                        self.last_error = Some(
                                                            "请先填写或选择恢复目标目录。"
                                                                .to_string(),
                                                        );
                                                        self.last_notice = None;
                                                    } else if let Err(error) =
                                                        open_path_in_explorer(&path)
                                                    {
                                                        self.last_error = Some(error.to_string());
                                                        self.last_notice = None;
                                                    }
                                                }
                                            });

                                            if ui
                                                .checkbox(
                                                    &mut self.skip_existing_restore_files,
                                                    "跳过已存在文件",
                                                )
                                                .changed()
                                            {
                                                let _ = self.persist_config();
                                            }

                                            if self.skip_existing_restore_files {
                                                ui.small(
                                                    "已启用跳过策略：目标目录中已有的同名文件会被保留。",
                                                );
                                            } else {
                                                ui.small(
                                                    "默认遇到同名文件会中止恢复，不会静默覆盖已有数据。",
                                                );
                                            }

                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                if ui.add(secondary_action_button("校验归档")).clicked()
                                                {
                                                    match archive::verify_archive(&loaded.path) {
                                                        Ok(result) => {
                                                            self.last_verification = Some(result);
                                                            self.last_error = None;
                                                        }
                                                        Err(error) => {
                                                            self.last_verification = None;
                                                            self.last_error =
                                                                Some(error.to_string());
                                                        }
                                                    }
                                                }

                                                let can_restore = !self
                                                    .restore_destination_input
                                                    .trim()
                                                    .is_empty()
                                                    && restore_summary.selected_file_count > 0
                                                    && self.background_task.is_none()
                                                    && matches!(
                                                        &restore_preflight,
                                                        Some(Ok(preflight))
                                                            if self.skip_existing_restore_files
                                                                || preflight.conflicting_files == 0
                                                    );
                                                if ui
                                                    .add_enabled(
                                                        can_restore,
                                                        primary_action_button("开始恢复"),
                                                    )
                                                    .clicked()
                                                {
                                                    let destination = PathBuf::from(
                                                        self.restore_destination_input.trim(),
                                                    );
                                                    self.start_restore_task(
                                                        loaded.path.clone(),
                                                        destination,
                                                        archive::RestoreSelection {
                                                            restore_user_data: self.restore_user_data,
                                                            restore_portable_apps: self
                                                                .restore_portable_apps,
                                                            restore_installed_app_dirs: self
                                                                .restore_installed_app_dirs,
                                                            selected_roots:
                                                                effective_restore_roots.clone(),
                                                            skip_existing_files: self
                                                                .skip_existing_restore_files,
                                                        },
                                                    );
                                                }
                                            });

                                            ui.add_space(8.0);
                                            if self.restore_destination_input.trim().is_empty() {
                                                compact_empty_state(
                                                    ui,
                                                    "还没有恢复目录",
                                                    "先选择恢复目标目录，再执行恢复。",
                                                );
                                            } else if restore_summary.selected_file_count == 0 {
                                                compact_empty_state(
                                                    ui,
                                                    "还没有恢复范围",
                                                    "先到“恢复范围”里选择至少一个已启用的根目录。",
                                                );
                                            } else if let Some(Err(error)) = &restore_preflight {
                                                status_banner(
                                                    ui,
                                                    Color32::from_rgb(252, 233, 229),
                                                    Color32::from_rgb(212, 122, 102),
                                                    &present_restore_error(&error.to_string()),
                                                );
                                            } else if let Some(Ok(preflight)) = &restore_preflight {
                                                ui.horizontal_wrapped(|ui| {
                                                    metric_tile(
                                                        ui,
                                                        Color32::from_rgb(245, 248, 252),
                                                        "将新增",
                                                        &preflight.new_files.to_string(),
                                                    );
                                                    metric_tile(
                                                        ui,
                                                        Color32::from_rgb(245, 248, 252),
                                                        if self.skip_existing_restore_files {
                                                            "将跳过"
                                                        } else {
                                                            "冲突文件"
                                                        },
                                                        &preflight.conflicting_files.to_string(),
                                                    );
                                                    metric_tile(
                                                        ui,
                                                        Color32::from_rgb(245, 248, 252),
                                                        "总计命中",
                                                        &preflight.selected_files.to_string(),
                                                    );
                                                });
                                                ui.add_space(8.0);
                                                if preflight.conflicting_files > 0 {
                                                    let example_text = if preflight
                                                        .conflict_examples
                                                        .is_empty()
                                                    {
                                                        String::new()
                                                    } else {
                                                        format!(
                                                            "\n示例：{}",
                                                            preflight.conflict_examples.join("；")
                                                        )
                                                    };
                                                    if self.skip_existing_restore_files {
                                                        status_banner(
                                                            ui,
                                                            Color32::from_rgb(247, 243, 230),
                                                            Color32::from_rgb(180, 150, 98),
                                                            &format!(
                                                                "预检：目标目录里已有 {} 个同名文件，本次会跳过这些文件，不会覆盖。{}",
                                                                preflight.conflicting_files,
                                                                example_text
                                                            ),
                                                        );
                                                    } else {
                                                        status_banner(
                                                            ui,
                                                            Color32::from_rgb(252, 233, 229),
                                                            Color32::from_rgb(212, 122, 102),
                                                            &format!(
                                                                "预检：目标目录里已有 {} 个同名文件。按当前策略，恢复会在遇到第一个冲突时中止。可以改目录，或启用“跳过已存在文件”。{}",
                                                                preflight.conflicting_files,
                                                                example_text
                                                            ),
                                                        );
                                                    }
                                                } else {
                                                    status_banner(
                                                        ui,
                                                        Color32::from_rgb(232, 239, 248),
                                                        Color32::from_rgb(130, 155, 186),
                                                        &format!(
                                                            "预检：将恢复 {} 个范围中的 {} 个文件，目标目录{}。",
                                                            restore_summary.selected_root_count,
                                                            preflight.selected_files,
                                                            if preflight.destination_exists
                                                                && preflight.destination_is_directory
                                                            {
                                                                "已存在且没有同名冲突"
                                                            } else {
                                                                "将由 WinRehome 自动创建"
                                                            }
                                                        ),
                                                    );
                                                }
                                                ui.add_space(8.0);
                                                if !preflight.new_examples.is_empty()
                                                    || !preflight.conflict_examples.is_empty()
                                                {
                                                    if ui.available_width() > 920.0 {
                                                        ui.columns(2, |columns| {
                                                            review_list_panel(
                                                                &mut columns[0],
                                                                "将新增的文件示例",
                                                                "这些文件路径在目标目录中还不存在。",
                                                                180.0,
                                                                |ui| {
                                                                    if preflight.new_examples.is_empty()
                                                                    {
                                                                        compact_empty_state(
                                                                            ui,
                                                                            "没有新增文件示例",
                                                                            "当前预检没有发现需要新建的文件路径。",
                                                                        );
                                                                    } else {
                                                                        for path in &preflight.new_examples
                                                                        {
                                                                            ui.small(path);
                                                                        }
                                                                    }
                                                                },
                                                            );
                                                            review_list_panel(
                                                                &mut columns[1],
                                                                if self.skip_existing_restore_files {
                                                                    "将跳过的文件示例"
                                                                } else {
                                                                    "冲突文件示例"
                                                                },
                                                                if self.skip_existing_restore_files {
                                                                    "这些文件已存在，按当前策略会被保留并跳过。"
                                                                } else {
                                                                    "这些文件已存在，按当前策略会阻止继续恢复。"
                                                                },
                                                                180.0,
                                                                |ui| {
                                                                    if preflight
                                                                        .conflict_examples
                                                                        .is_empty()
                                                                    {
                                                                        compact_empty_state(
                                                                            ui,
                                                                            "没有冲突文件示例",
                                                                            "当前预检没有发现同名目标文件。",
                                                                        );
                                                                    } else {
                                                                        for path in
                                                                            &preflight.conflict_examples
                                                                        {
                                                                            ui.small(path);
                                                                        }
                                                                    }
                                                                },
                                                            );
                                                        });
                                                    } else {
                                                        review_list_panel(
                                                            ui,
                                                            "预检差异示例",
                                                            "快速查看会新增和会冲突的目标路径。",
                                                            220.0,
                                                            |ui| {
                                                                if preflight.new_examples.is_empty()
                                                                    && preflight
                                                                        .conflict_examples
                                                                        .is_empty()
                                                                {
                                                                    compact_empty_state(
                                                                        ui,
                                                                        "没有可展示的差异示例",
                                                                        "当前预检没有收集到差异示例。",
                                                                    );
                                                                } else {
                                                                    if !preflight.new_examples.is_empty()
                                                                    {
                                                                        ui.label(
                                                                            RichText::new(
                                                                                "将新增",
                                                                            )
                                                                            .strong(),
                                                                        );
                                                                        for path in
                                                                            &preflight.new_examples
                                                                        {
                                                                            ui.small(path);
                                                                        }
                                                                        ui.add_space(8.0);
                                                                    }
                                                                    if !preflight
                                                                        .conflict_examples
                                                                        .is_empty()
                                                                    {
                                                                        ui.label(
                                                                            RichText::new(
                                                                                if self
                                                                                    .skip_existing_restore_files
                                                                                {
                                                                                    "将跳过"
                                                                                } else {
                                                                                    "冲突文件"
                                                                                },
                                                                            )
                                                                            .strong(),
                                                                        );
                                                                        for path in
                                                                            &preflight.conflict_examples
                                                                        {
                                                                            ui.small(path);
                                                                        }
                                                                    }
                                                                }
                                                            },
                                                        );
                                                    }
                                                }
                                            } else {
                                                status_banner(
                                                    ui,
                                                    Color32::from_rgb(232, 239, 248),
                                                    Color32::from_rgb(130, 155, 186),
                                                    &format!(
                                                        "将恢复 {} 个范围中的 {} 个文件，目标目录：{}",
                                                        restore_summary.selected_root_count,
                                                        restore_summary.selected_file_count,
                                                        self.restore_destination_input.trim()
                                                    ),
                                                );
                                            }
                                        },
                                    );
                                }
                            }
                        } else {
                            restore_workflow_switcher(ui, false, None);
                            ui.add_space(8.0);
                            card_panel(
                                ui,
                                "恢复备份",
                                "",
                                |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "归档大小",
                                            "等待加载",
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "已安装软件记录",
                                            "等待加载",
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "个人文件目录",
                                            "等待加载",
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 248, 252),
                                            "便携软件",
                                            "等待加载",
                                        );
                                    });

                                    ui.add_space(10.0);
                                    ui.label(RichText::new("步骤 1：归档文件").strong());
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(
                                                    &mut self.archive_path_input,
                                                )
                                                .desired_width(360.0)
                                                .hint_text("选择或输入 .wrh 归档路径"),
                                            )
                                            .changed()
                                        {
                                            let _ = self.persist_config();
                                        }
                                        if ui.add(secondary_action_button("浏览归档")).clicked() {
                                            if let Some(path) =
                                                pick_archive_file_from_input(&self.archive_path_input)
                                            {
                                                self.load_archive_from_path(path);
                                            }
                                        }
                                        if ui.add(secondary_action_button("加载归档")).clicked() {
                                            let path =
                                                PathBuf::from(self.archive_path_input.trim());
                                            self.load_archive_from_path(path);
                                        }
                                        if ui
                                            .add_enabled(
                                                self.recent_archives.first().is_some(),
                                                secondary_action_button("加载最近一个"),
                                            )
                                            .clicked()
                                        {
                                            if let Some(path) = self.recent_archives.first().cloned() {
                                                self.load_archive_from_path(path);
                                            }
                                        }
                                    });

                                    if !self.recent_archives.is_empty() {
                                        let recent_archive_buttons: Vec<PathBuf> =
                                            self.recent_archives.iter().take(4).cloned().collect();
                                        ui.add_space(8.0);
                                        ui.label(RichText::new("最近发现的归档").strong());
                                        ui.horizontal_wrapped(|ui| {
                                            for path in recent_archive_buttons {
                                                let label = path
                                                    .file_name()
                                                    .and_then(|value| value.to_str())
                                                    .unwrap_or("未知归档");
                                                if ui
                                                    .add(secondary_action_button(label))
                                                    .clicked()
                                                {
                                                    self.load_archive_from_path(path.clone());
                                                }
                                            }
                                        });
                                    }

                                    ui.add_space(8.0);
                                    if ui.available_width() > 920.0 {
                                        ui.columns(3, |columns| {
                                            review_list_panel(
                                                &mut columns[0],
                                                "个人文件",
                                                "",
                                                220.0,
                                                |ui| {
                                                    compact_empty_state(
                                                        ui,
                                                        "等待归档内容",
                                                        "",
                                                    );
                                                },
                                            );
                                            review_list_panel(
                                                &mut columns[1],
                                                "便携软件",
                                                "",
                                                220.0,
                                                |ui| {
                                                    compact_empty_state(
                                                        ui,
                                                        "等待归档内容",
                                                        "",
                                                    );
                                                },
                                            );
                                            review_list_panel(
                                                &mut columns[2],
                                                "已安装软件记录",
                                                "",
                                                220.0,
                                                |ui| {
                                                    compact_empty_state(
                                                        ui,
                                                        "等待归档内容",
                                                        "",
                                                    );
                                                },
                                            );
                                        });
                                    } else {
                                        review_list_panel(
                                            ui,
                                            "个人文件",
                                            "",
                                            220.0,
                                            |ui| {
                                                compact_empty_state(
                                                    ui,
                                                    "等待归档内容",
                                                    "",
                                                );
                                            },
                                        );
                                        ui.add_space(8.0);
                                        review_list_panel(
                                            ui,
                                            "便携软件",
                                            "",
                                            220.0,
                                            |ui| {
                                                compact_empty_state(
                                                    ui,
                                                    "等待归档内容",
                                                    "",
                                                );
                                            },
                                        );
                                        ui.add_space(8.0);
                                        review_list_panel(
                                            ui,
                                            "已安装软件记录",
                                            "",
                                            220.0,
                                            |ui| {
                                                compact_empty_state(
                                                    ui,
                                                    "等待归档内容",
                                                    "",
                                                );
                                            },
                                        );
                                    }

                                    ui.add_space(8.0);
                                    card_panel(
                                        ui,
                                        "步骤 3：恢复设置",
                                        "",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if ui
                                                    .add(
                                                        egui::TextEdit::singleline(
                                                            &mut self.restore_destination_input,
                                                        )
                                                        .desired_width(360.0)
                                                        .hint_text("例如 D:\\WinRehome Restore"),
                                                    )
                                                    .changed()
                                                {
                                                    let _ = self.persist_config();
                                                }
                                                if ui.add(secondary_action_button("浏览目录")).clicked()
                                                {
                                                    if let Some(path) = pick_folder_from_input(
                                                        &self.restore_destination_input,
                                                    ) {
                                                        self.restore_destination_input =
                                                            path.display().to_string();
                                                        let _ = self.persist_config();
                                                    }
                                                }
                                            });
                                            ui.add_space(8.0);
                                            compact_empty_state(
                                                ui,
                                                "等待归档加载",
                                                "",
                                            );
                                        },
                                    );
                                },
                            );
                        }
                    }
                });
            };
            if use_main_scroll {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| render_main(ui));
            } else {
                render_main(ui);
            }
        });

        render_error_dialog(ctx, self);
        if let Some(task) = &self.background_task {
            show_progress_dialog(ctx, &task.progress);
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let _ = self.persist_config();
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.persist_config();
    }
}

fn card_panel(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(252, 253, 255))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(211, 218, 228)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    RichText::new(title)
                        .size(19.0)
                        .strong()
                        .color(Color32::from_rgb(32, 39, 49)),
                );
            }
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).color(Color32::from_rgb(89, 97, 111)));
                ui.add_space(8.0);
            }
            add_contents(ui);
        });
}

fn render_overview_card(ui: &mut egui::Ui, app: &mut WinRehomeApp) {
    egui::Frame::new()
        .fill(Color32::from_rgb(252, 253, 255))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(211, 218, 228)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(28, 26))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("欢迎使用 WinRehome")
                        .size(28.0)
                        .strong()
                        .color(Color32::from_rgb(32, 39, 49)),
                );
                ui.add_space(14.0);
                ui.label(
                    RichText::new(
                        "一个面向 Windows 的迁移备份工具，目标是在尽可能节省备份空间的前提下，保留真正有迁移价值的个人数据。",
                    )
                    .size(17.0)
                    .color(Color32::from_rgb(89, 97, 111)),
                );
                ui.add_space(22.0);
                ui.label(
                    RichText::new(format!("当前版本：{}", env!("CARGO_PKG_VERSION")))
                        .size(15.5)
                        .color(Color32::from_rgb(70, 79, 92)),
                );
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(
                        RichText::new("GitHub 仓库").size(16.0),
                        GITHUB_REPO_URL,
                    );
                    ui.add_space(18.0);
                    ui.hyperlink_to(RichText::new("反馈地址").size(16.0), FEEDBACK_URL);
                });
                ui.add_space(18.0);
                if ui
                    .checkbox(
                        &mut app.remember_window_geometry,
                        RichText::new("记忆上次窗口大小和位置").size(16.0),
                    )
                    .changed()
                {
                    let _ = app.persist_config();
                }
            });
        });
}

fn render_scan_scope_page(ui: &mut egui::Ui, app: &mut WinRehomeApp) {
    card_panel(ui, "", "", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !matches!(
                        app.background_task,
                        Some(BackgroundTaskState {
                            kind: BackgroundTaskKind::Scan,
                            ..
                        })
                    ),
                    primary_action_button("开始扫描当前系统"),
                )
                .clicked()
            {
                app.start_scan_preview();
            }
            if ui.add(secondary_action_button("恢复默认")).clicked() {
                app.scan_roots = default_scan_root_entries();
                let _ = app.persist_config();
            }
            if ui.add(secondary_action_button("增加路径")).clicked() {
                app.scan_roots.push(ScanRootEntry {
                    path: String::new(),
                });
                let _ = app.persist_config();
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("扫描路径列表").strong());
        ui.add_space(8.0);

        if app.scan_roots.is_empty() {
            compact_empty_state(
                ui,
                "还没有扫描路径",
                "可以手动增加路径，或点击“恢复默认”重新载入默认扫描路径。",
            );
            return;
        }

        let mut remove_index = None;
        let mut scan_roots_dirty = false;
        for (index, entry) in app.scan_roots.iter_mut().enumerate() {
            egui::Frame::new()
                .fill(Color32::from_rgb(249, 251, 254))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(212, 220, 231)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(format!("路径 {}", index + 1))
                            .strong()
                            .color(Color32::from_rgb(49, 59, 72)),
                    );
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        let input_width = (ui.available_width() - 236.0).max(280.0);
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut entry.path)
                                    .desired_width(input_width)
                                    .hint_text("例如 D:\\ 或 D:\\Tools"),
                            )
                            .changed()
                        {
                            scan_roots_dirty = true;
                        }
                        if ui.add(secondary_action_button("浏览")).clicked() {
                            if let Some(path) = pick_folder_from_input(&entry.path) {
                                entry.path = path.display().to_string();
                                scan_roots_dirty = true;
                            }
                        }
                        if ui.add(secondary_action_button("删除")).clicked() {
                            remove_index = Some(index);
                        }
                    });
                });
            ui.add_space(6.0);
        }

        if let Some(index) = remove_index {
            app.scan_roots.remove(index);
            scan_roots_dirty = true;
        }

        if scan_roots_dirty {
            let _ = app.persist_config();
        }

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("排除路径列表").strong());
            if ui.add(secondary_action_button("增加排除路径")).clicked() {
                app.excluded_scan_roots.push(ScanRootEntry {
                    path: String::new(),
                });
                let _ = app.persist_config();
            }
        });
        ui.add_space(8.0);

        if app.excluded_scan_roots.is_empty() {
            compact_empty_state(
                ui,
                "还没有排除路径",
                "加入排除路径后，扫描便携软件时会跳过这些目录。",
            );
            return;
        }

        let mut remove_excluded_index = None;
        let mut excluded_roots_dirty = false;
        for (index, entry) in app.excluded_scan_roots.iter_mut().enumerate() {
            egui::Frame::new()
                .fill(Color32::from_rgb(252, 248, 246))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(225, 214, 206)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(format!("排除路径 {}", index + 1))
                            .strong()
                            .color(Color32::from_rgb(88, 64, 47)),
                    );
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        let input_width = (ui.available_width() - 236.0).max(280.0);
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut entry.path)
                                    .desired_width(input_width)
                                    .hint_text("例如 D:\\Games 或 D:\\Downloads"),
                            )
                            .changed()
                        {
                            excluded_roots_dirty = true;
                        }
                        if ui.add(secondary_action_button("浏览")).clicked() {
                            if let Some(path) = pick_folder_from_input(&entry.path) {
                                entry.path = path.display().to_string();
                                excluded_roots_dirty = true;
                            }
                        }
                        if ui.add(secondary_action_button("删除")).clicked() {
                            remove_excluded_index = Some(index);
                        }
                    });
                });
            ui.add_space(6.0);
        }

        if let Some(index) = remove_excluded_index {
            app.excluded_scan_roots.remove(index);
            excluded_roots_dirty = true;
        }

        if excluded_roots_dirty {
            let _ = app.persist_config();
        }
    });
}

fn backup_workflow_switcher(
    ui: &mut egui::Ui,
    active_page: &mut BackupWorkflowPage,
    user_data_enabled: bool,
    portable_enabled: bool,
    installed_enabled: bool,
    output_enabled: bool,
) {
    ui.horizontal(|ui| {
        let button_width = 152.0;
        let arrow_width = 18.0;
        let item_spacing = ui.spacing().item_spacing.x;
        let total_width = button_width * 5.0 + arrow_width * 4.0 + item_spacing * 8.0;
        let leading_space = ((ui.available_width() - total_width) * 0.5).max(0.0);
        if leading_space > 0.0 {
            ui.add_space(leading_space);
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;
            if segment_button_enabled(
                ui,
                *active_page == BackupWorkflowPage::ScanScope,
                true,
                "① 选择扫描范围",
            ) {
                *active_page = BackupWorkflowPage::ScanScope;
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if segment_button_enabled(
                ui,
                *active_page == BackupWorkflowPage::UserData,
                user_data_enabled,
                "② 筛选个人文件",
            ) {
                *active_page = BackupWorkflowPage::UserData;
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if segment_button_enabled(
                ui,
                *active_page == BackupWorkflowPage::PortableApps,
                portable_enabled,
                "③ 选择便携软件",
            ) {
                *active_page = BackupWorkflowPage::PortableApps;
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if segment_button_enabled(
                ui,
                *active_page == BackupWorkflowPage::InstalledApps,
                installed_enabled,
                "④ 选择已安装软件",
            ) {
                *active_page = BackupWorkflowPage::InstalledApps;
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if segment_button_enabled(
                ui,
                *active_page == BackupWorkflowPage::Output,
                output_enabled,
                "⑤ 生成备份",
            ) {
                *active_page = BackupWorkflowPage::Output;
            }
        });
    });
}

fn restore_workflow_switcher(
    ui: &mut egui::Ui,
    loaded: bool,
    mut active_section: Option<&mut RestoreSection>,
) {
    ui.horizontal(|ui| {
        let button_width = 152.0;
        let arrow_width = 18.0;
        let item_spacing = ui.spacing().item_spacing.x;
        let total_width = button_width * 4.0 + arrow_width * 3.0 + item_spacing * 6.0;
        let leading_space = ((ui.available_width() - total_width) * 0.5).max(0.0);
        if leading_space > 0.0 {
            ui.add_space(leading_space);
        }

        let current_section = active_section.as_deref().copied();
        let current_step = if !loaded {
            0
        } else {
            match current_section.unwrap_or(RestoreSection::InstalledApps) {
                RestoreSection::InstalledApps => 1,
                RestoreSection::RestoreScope => 2,
                RestoreSection::RestoreAction => 3,
            }
        };

        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = item_spacing;
            let _ = step_segment_button(ui, current_step == 0, true, "① 选择备份");
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if let Some(active_section) = active_section.as_deref_mut() {
                if step_segment_button(ui, current_step == 1, loaded, "② 查看备份内容") {
                    *active_section = RestoreSection::InstalledApps;
                }
            } else {
                let _ = step_segment_button(ui, current_step == 1, false, "② 查看备份内容");
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if let Some(active_section) = active_section.as_deref_mut() {
                if step_segment_button(ui, current_step == 2, loaded, "③ 选择恢复范围") {
                    *active_section = RestoreSection::RestoreScope;
                }
            } else {
                let _ = step_segment_button(ui, current_step == 2, false, "③ 选择恢复范围");
            }
            ui.label(
                RichText::new("→")
                    .size(16.0)
                    .color(Color32::from_rgb(135, 145, 158)),
            );
            if let Some(active_section) = active_section.as_deref_mut() {
                if step_segment_button(ui, current_step == 3, loaded, "④ 执行恢复") {
                    *active_section = RestoreSection::RestoreAction;
                }
            } else {
                let _ = step_segment_button(ui, current_step == 3, false, "④ 执行恢复");
            }
        });
    });
}

fn step_segment_button(ui: &mut egui::Ui, selected: bool, enabled: bool, label: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).strong().color(if selected {
            Color32::WHITE
        } else {
            Color32::from_rgb(42, 52, 64)
        }))
        .fill(if selected {
            Color32::from_rgb(0, 103, 192)
        } else {
            Color32::from_rgb(244, 247, 251)
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                Color32::from_rgb(0, 79, 150)
            } else {
                Color32::from_rgb(198, 207, 219)
            },
        ))
        .corner_radius(egui::CornerRadius::same(10))
        .min_size(egui::vec2(152.0, 34.0)),
    )
    .clicked()
}

fn segment_button_enabled(ui: &mut egui::Ui, selected: bool, enabled: bool, label: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).strong().color(if selected {
            Color32::WHITE
        } else if enabled {
            Color32::from_rgb(42, 52, 64)
        } else {
            Color32::from_rgb(132, 140, 152)
        }))
        .fill(if selected {
            Color32::from_rgb(0, 103, 192)
        } else if enabled {
            Color32::from_rgb(244, 247, 251)
        } else {
            Color32::from_rgb(240, 243, 247)
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                Color32::from_rgb(0, 79, 150)
            } else {
                Color32::from_rgb(198, 207, 219)
            },
        ))
        .corner_radius(egui::CornerRadius::same(10))
        .min_size(egui::vec2(152.0, 34.0)),
    )
    .clicked()
}

fn render_scan_installed_apps_panel(
    ui: &mut egui::Ui,
    preview: &plan::BackupPreview,
    scan_filter: &mut String,
    filtered_installed_count: usize,
    filtered_installed_backup_available_count: usize,
    filtered_installed_backup_count: usize,
    filtered_scan_apps: &[InstalledAppExportRow],
    selected_installed_app_dirs: &mut HashSet<String>,
    backup_output_input: &str,
    last_notice: &mut Option<String>,
    last_error: &mut Option<String>,
) -> bool {
    let mut installed_app_dirs_dirty = false;
    let visible_installed_keys: Vec<String> = preview
        .installed_apps
        .iter()
        .filter(|app| {
            if !app.can_backup_files() {
                return false;
            }
            let install_location = app
                .install_location
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            matches_filter(
                scan_filter.as_str(),
                &[
                    &app.display_name,
                    app.source,
                    &app.uninstall_key,
                    &install_location,
                ],
            )
        })
        .map(|app| app.selection_key())
        .collect();

    card_panel(ui, "", "", |ui| {
        search_toolbar(ui, scan_filter, "输入名称或路径搜索已安装软件");
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            section_counter(ui, "筛选命中", filtered_installed_count);
            section_counter(ui, "可备份目录", filtered_installed_backup_available_count);
            section_counter(ui, "已备份文件", filtered_installed_backup_count);
            if ui.add(secondary_action_button("全选命中")).clicked() {
                for key in &visible_installed_keys {
                    selected_installed_app_dirs.insert(key.clone());
                }
                installed_app_dirs_dirty = true;
            }
            if ui.add(secondary_action_button("反选命中")).clicked() {
                for key in &visible_installed_keys {
                    if !selected_installed_app_dirs.remove(key) {
                        selected_installed_app_dirs.insert(key.clone());
                    }
                }
                installed_app_dirs_dirty = true;
            }
            if ui.add(secondary_action_button("清空命中")).clicked() {
                for key in &visible_installed_keys {
                    selected_installed_app_dirs.remove(key);
                }
                installed_app_dirs_dirty = true;
            }
            if ui.add(secondary_action_button("导出命中 CSV")).clicked() {
                let default_name = "WinRehome-installed-apps-scan.csv";
                match pick_inventory_export_path(default_name, Some(backup_output_input)) {
                    Some(path) => {
                        match export_installed_app_inventory_csv(&path, filtered_scan_apps) {
                            Ok(count) => {
                                *last_notice = Some(format!(
                                    "软件记录已导出：{}，共 {} 条。",
                                    path.display(),
                                    count
                                ));
                                *last_error = None;
                            }
                            Err(error) => {
                                *last_error = Some(error.to_string());
                                *last_notice = None;
                            }
                        }
                    }
                    None if filtered_scan_apps.is_empty() => {
                        *last_error = Some("当前没有可导出的软件记录。".to_string());
                        *last_notice = None;
                    }
                    None => {}
                }
            }
        });
        ui.add_space(8.0);
        if filtered_installed_count == 0 {
            compact_empty_state(ui, "没有命中的软件记录", "可以调整筛选词后继续审查。");
        } else {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 620.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for app in preview.installed_apps.iter().take(120) {
                                let install_location = app
                                    .install_location
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_default();
                                if !matches_filter(
                                    scan_filter.as_str(),
                                    &[
                                        &app.display_name,
                                        app.source,
                                        &app.uninstall_key,
                                        &install_location,
                                    ],
                                ) {
                                    continue;
                                }
                                let key = app.selection_key();
                                let backup_files = selected_installed_app_dirs.contains(&key);
                                let source_path = app
                                    .install_location
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "-".to_string());
                                let estimated_size = app
                                    .install_stats
                                    .map(|stats| format_bytes(stats.total_bytes))
                                    .unwrap_or_else(|| "未知".to_string());
                                result_card_with_badge(
                                    ui,
                                    path_kind_badge_from_path(app.install_location.as_deref()),
                                    &app.display_name,
                                    "",
                                    |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            if app.can_backup_files() {
                                                if ui
                                                    .selectable_label(!backup_files, "仅备份记录")
                                                    .clicked()
                                                {
                                                    selected_installed_app_dirs.remove(&key);
                                                    installed_app_dirs_dirty = true;
                                                }
                                                if ui
                                                    .selectable_label(backup_files, "备份文件")
                                                    .clicked()
                                                {
                                                    selected_installed_app_dirs.insert(key.clone());
                                                    installed_app_dirs_dirty = true;
                                                }
                                            } else {
                                                ui.small(
                                                    RichText::new("仅备份记录")
                                                        .color(Color32::from_rgb(97, 106, 118)),
                                                );
                                            }
                                            if ui
                                                .add_enabled(
                                                    app.install_location.is_some(),
                                                    secondary_action_button("打开所在路径"),
                                                )
                                                .clicked()
                                            {
                                                if let Some(path) = &app.install_location {
                                                    if let Err(error) =
                                                        open_containing_path_in_explorer(path)
                                                    {
                                                        *last_error = Some(error.to_string());
                                                        *last_notice = None;
                                                    }
                                                }
                                            }
                                        });
                                        detail_line(ui, format!("来源路径：{}", source_path));
                                        detail_line(ui, "主程序：未提供");
                                        detail_line(ui, format!("预计大小：{}", estimated_size));
                                    },
                                );
                                ui.add_space(6.0);
                            }
                        });
                },
            );
        }
    });

    installed_app_dirs_dirty
}

fn open_path_in_explorer(target: &Path) -> anyhow::Result<()> {
    let mut command = Command::new("explorer");
    if target.is_file() {
        command.arg(format!("/select,{}", target.display()));
    } else {
        command.arg(target);
    }

    command.spawn().map(|_| ()).map_err(|error| {
        anyhow::anyhow!(
            "failed to open Explorer for {}: {}",
            target.display(),
            error
        )
    })
}

fn open_containing_path_in_explorer(target: &Path) -> anyhow::Result<()> {
    if target.is_dir() {
        return Command::new("explorer")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to open Explorer for {}: {}",
                    target.display(),
                    error
                )
            });
    }

    if target.is_file() {
        return Command::new("explorer")
            .arg(format!("/select,\"{}\"", target.display()))
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to reveal file in Explorer for {}: {}",
                    target.display(),
                    error
                )
            });
    }

    let folder = target
        .parent()
        .map(|parent| parent.to_path_buf())
        .unwrap_or_else(|| target.to_path_buf());

    Command::new("explorer")
        .arg(&folder)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to open Explorer for {}: {}",
                folder.display(),
                error
            )
        })
}

fn pick_inventory_export_path(default_name: &str, directory_hint: Option<&str>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_file_name(default_name);
    if let Some(path) = directory_hint.and_then(path_for_picker) {
        dialog = dialog.set_directory(path);
    }
    dialog.save_file()
}

fn export_installed_app_inventory_csv(
    output_path: &Path,
    rows: &[InstalledAppExportRow],
) -> anyhow::Result<usize> {
    if rows.is_empty() {
        anyhow::bail!("there are no installed-app records to export");
    }

    let mut csv = String::from("DisplayName,Source,InstallLocation,RegistryKey\n");
    for row in rows {
        csv.push_str(&escape_csv_field(&row.display_name));
        csv.push(',');
        csv.push_str(&escape_csv_field(&row.source));
        csv.push(',');
        csv.push_str(&escape_csv_field(
            row.install_location.as_deref().unwrap_or_default(),
        ));
        csv.push(',');
        csv.push_str(&escape_csv_field(&row.uninstall_key));
        csv.push('\n');
    }

    fs::write(output_path, csv).map_err(|error| {
        anyhow::anyhow!(
            "failed to write installed-app inventory {}: {}",
            output_path.display(),
            error
        )
    })?;
    Ok(rows.len())
}

fn escape_csv_field(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    if escaped.contains([',', '"', '\n', '\r']) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

fn pick_folder_from_input(current_value: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(path) = path_for_picker(current_value) {
        dialog = dialog.set_directory(path);
    }
    dialog.pick_folder()
}

fn pick_archive_file_from_input(current_value: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().add_filter("WinRehome Archive", &["wrh"]);
    if let Some(path) = path_for_picker(current_value) {
        dialog = dialog.set_directory(path);
    }
    dialog.pick_file()
}

fn path_for_picker(current_value: &str) -> Option<PathBuf> {
    let trimmed = current_value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    if path.is_dir() {
        Some(path)
    } else {
        path.parent().map(|parent| parent.to_path_buf())
    }
}

fn default_scan_root_entries() -> Vec<ScanRootEntry> {
    plan::default_scan_roots()
        .into_iter()
        .map(|path| ScanRootEntry {
            path: path.display().to_string(),
        })
        .collect()
}

fn first_available_backup_page(preview: &plan::BackupPreview) -> BackupWorkflowPage {
    if !preview.user_data_roots.is_empty() {
        BackupWorkflowPage::UserData
    } else if !preview.portable_candidates.is_empty() {
        BackupWorkflowPage::PortableApps
    } else if !preview.installed_apps.is_empty() {
        BackupWorkflowPage::InstalledApps
    } else {
        BackupWorkflowPage::ScanScope
    }
}

fn configured_path_entries(entries: &[ScanRootEntry]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    for entry in entries {
        let trimmed = entry.path.trim();
        if trimmed.is_empty() {
            continue;
        }

        let path = PathBuf::from(trimmed);
        if !path.exists() {
            continue;
        }

        let key = plan::path_key(&path);
        if seen.insert(key) {
            roots.push(path);
        }
    }

    collapse_nested_paths(roots)
}

fn collapse_nested_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| plan::path_key(left).cmp(&plan::path_key(right)))
    });

    let mut collapsed = Vec::new();
    for path in paths {
        if collapsed
            .iter()
            .any(|existing: &PathBuf| path == *existing || path.starts_with(existing))
        {
            continue;
        }
        collapsed.push(path);
    }
    collapsed
}

fn saved_path_entries(entries: &[ScanRootEntry]) -> Vec<config::SavedScanRoot> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = entry.path.trim();
            (!path.is_empty()).then(|| config::SavedScanRoot {
                path: path.to_string(),
                enabled: true,
            })
        })
        .collect()
}

fn optional_dir_from_input(current_value: &str) -> Option<PathBuf> {
    let trimmed = current_value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    if path.is_dir() { Some(path) } else { None }
}

fn optional_parent_dir_from_input(current_value: &str) -> Option<PathBuf> {
    let trimmed = current_value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    path.parent().map(|parent| parent.to_path_buf())
}

fn dedupe_dirs(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for dir in dirs {
        let key = dir.display().to_string().to_lowercase();
        if seen.insert(key) {
            deduped.push(dir);
        }
    }

    deduped
}

fn centered_content(ui: &mut egui::Ui, max_width: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    let available_width = ui.available_width();
    let target_width = available_width.min(max_width);
    let side_space = ((available_width - target_width) / 2.0).max(0.0);

    ui.horizontal(|ui| {
        if side_space > 0.0 {
            ui.add_space(side_space);
        }
        ui.scope(|ui| {
            ui.set_min_width(target_width);
            ui.set_max_width(target_width);
            ui.vertical(|ui| {
                add_contents(ui);
            });
        });
    });
}

fn show_feedback_banners(ui: &mut egui::Ui, app: &WinRehomeApp) {
    if let Some(notice) = &app.last_notice {
        status_banner(
            ui,
            Color32::from_rgb(232, 239, 248),
            Color32::from_rgb(108, 143, 184),
            notice,
        );
        ui.add_space(8.0);
    }
    if let Some(result) = &app.last_archive {
        status_banner(
            ui,
            Color32::from_rgb(230, 242, 233),
            Color32::from_rgb(102, 150, 113),
            &format!(
                "归档已创建并完成校验：{}\n{} 个文件，原始大小 {}，归档大小 {}。",
                result.archive_path.display(),
                result.file_count,
                format_bytes(result.original_bytes),
                format_bytes(result.stored_bytes)
            ),
        );
        ui.add_space(8.0);
    }
    if let Some(result) = &app.last_restore {
        status_banner(
            ui,
            Color32::from_rgb(232, 239, 248),
            Color32::from_rgb(97, 128, 177),
            &format!(
                "归档已恢复：{} -> {}\n{} 个文件，{}，跳过 {} 个已存在文件。",
                result.archive_path.display(),
                result.destination_root.display(),
                result.restored_files,
                format_bytes(result.restored_bytes),
                result.skipped_existing_files
            ),
        );
        ui.add_space(8.0);
    }
    if let Some(result) = &app.last_verification {
        status_banner(
            ui,
            Color32::from_rgb(241, 235, 250),
            Color32::from_rgb(133, 104, 188),
            &format!(
                "归档校验完成：{}\n{} 个文件，{}。",
                result.archive_path.display(),
                result.verified_files,
                format_bytes(result.verified_bytes)
            ),
        );
        ui.add_space(8.0);
    }
}

fn render_error_dialog(ctx: &egui::Context, app: &mut WinRehomeApp) {
    let Some(error) = app.last_error.clone() else {
        return;
    };

    egui::Area::new(egui::Id::new("error_dialog"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(252, 253, 255))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(212, 122, 102)))
                .corner_radius(egui::CornerRadius::same(14))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(12, 18, 28, 60),
                })
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.label(
                        RichText::new("操作失败")
                            .size(18.0)
                            .strong()
                            .color(Color32::from_rgb(173, 67, 49)),
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new(error).color(Color32::from_rgb(56, 64, 75)));
                    ui.add_space(14.0);
                    if ui.add(primary_action_button("关闭")).clicked() {
                        app.last_error = None;
                    }
                });
        });
}

fn show_progress_dialog(ctx: &egui::Context, progress: &BackgroundTaskProgress) {
    let layer_id = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("task_overlay"));
    let painter = ctx.layer_painter(layer_id);
    let screen_rect = ctx.screen_rect();
    painter.rect_filled(
        screen_rect,
        0.0,
        Color32::from_rgba_unmultiplied(18, 24, 32, 140),
    );

    egui::Area::new(egui::Id::new("task_overlay_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let _ = ui.allocate_rect(
                egui::Rect::from_min_size(screen_rect.min, screen_rect.size()),
                egui::Sense::click(),
            );
        });

    egui::Area::new(egui::Id::new("task_progress_dialog"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(252, 253, 255))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(197, 208, 222)))
                .corner_radius(egui::CornerRadius::same(14))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(12, 18, 28, 60),
                })
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.label(RichText::new(&progress.title).size(18.0).strong());
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(22.0));
                        ui.label(
                            RichText::new(match progress.kind {
                                BackgroundTaskKind::Scan => "系统内容扫描中",
                                BackgroundTaskKind::Restore => "文件恢复中",
                            })
                            .strong(),
                        );
                    });
                    ui.add_space(10.0);
                    ui.add(
                        egui::ProgressBar::new(progress.fraction.clamp(0.0, 1.0))
                            .desired_width(ui.available_width())
                            .show_percentage(),
                    );
                    ui.add_space(8.0);
                    ui.label(&progress.detail);
                });
        });
}

fn workspace_button(
    ui: &mut egui::Ui,
    active_workspace: &mut WorkspaceView,
    target: WorkspaceView,
    label: &str,
    enabled: bool,
) {
    let selected = *active_workspace == target;
    let button = egui::Button::new(RichText::new(label).size(16.0).strong().color(if selected {
        Color32::WHITE
    } else {
        Color32::from_rgb(42, 52, 64)
    }))
    .fill(if selected {
        Color32::from_rgb(0, 103, 192)
    } else {
        Color32::from_rgb(244, 247, 251)
    })
    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(198, 207, 219)))
    .corner_radius(egui::CornerRadius::same(10))
    .min_size(egui::vec2(132.0, 40.0));

    if ui.add_enabled(enabled, button).clicked() {
        *active_workspace = target;
    }
}

fn primary_action_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
        .fill(Color32::from_rgb(0, 103, 192))
        .min_size(egui::vec2(120.0, 34.0))
}

fn secondary_action_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::from_rgb(45, 56, 70)))
        .fill(Color32::from_rgb(245, 248, 252))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(198, 207, 219)))
        .min_size(egui::vec2(108.0, 34.0))
}

fn review_list_panel(
    ui: &mut egui::Ui,
    title: &str,
    meta: &str,
    min_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(252, 253, 255))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(214, 220, 230)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            let panel_height = ui.available_height().max(min_height);
            ui.set_min_height(panel_height);
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(39, 47, 58)),
            );
            if !meta.is_empty() {
                ui.small(RichText::new(meta).color(Color32::from_rgb(93, 103, 116)));
            }
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(panel_height)
                .show(ui, |ui| add_contents(ui));
        });
}

fn metric_tile(ui: &mut egui::Ui, fill: Color32, label: &str, value: &str) {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(211, 218, 228)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .size(12.0)
                    .color(Color32::from_rgb(96, 105, 118)),
            );
            ui.label(
                RichText::new(value)
                    .size(18.0)
                    .strong()
                    .color(Color32::from_rgb(33, 42, 55)),
            );
        });
}

fn search_toolbar(ui: &mut egui::Ui, filter: &mut String, hint: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(244, 248, 253))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(190, 204, 220)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("搜索")
                        .strong()
                        .color(Color32::from_rgb(43, 54, 68)),
                );
                let input_width = (ui.available_width() - 12.0).max(240.0);
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .desired_width(input_width)
                        .hint_text(hint),
                );
            });
        });
}

fn status_banner(ui: &mut egui::Ui, fill: Color32, stroke: Color32, text: &str) {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(text).color(Color32::from_rgb(45, 55, 68)));
        });
}

fn compact_empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(247, 249, 252))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(216, 223, 232)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(60, 70, 83)),
            );
            if !body.is_empty() {
                ui.small(body);
            }
        });
}

fn waiting_scan_result_state(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(Color32::from_rgb(247, 249, 252))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(216, 223, 232)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("等待扫描结果")
                        .size(22.0)
                        .strong()
                        .color(Color32::from_rgb(60, 70, 83)),
                );
            });
        });
}

fn section_counter(ui: &mut egui::Ui, label: &str, count: usize) {
    egui::Frame::new()
        .fill(Color32::from_rgb(243, 247, 252))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(208, 216, 226)))
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small(RichText::new(label).color(Color32::from_rgb(90, 101, 114)));
                ui.label(
                    RichText::new(count.to_string())
                        .strong()
                        .color(Color32::from_rgb(34, 44, 56)),
                );
            });
        });
}

fn result_card(ui: &mut egui::Ui, title: &str, meta: &str, add_extra: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(248, 250, 247))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(218, 226, 219)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(title).strong());
            if !meta.is_empty() {
                ui.small(meta);
            }
            add_extra(ui);
        });
}

fn result_card_with_badge(
    ui: &mut egui::Ui,
    badge: (&'static str, Color32, Color32),
    title: &str,
    meta: &str,
    add_extra: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(248, 250, 247))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(218, 226, 219)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                kind_badge(ui, badge);
                ui.label(RichText::new(title).strong());
            });
            if !meta.is_empty() {
                ui.small(meta);
            }
            add_extra(ui);
        });
}

fn selection_result_card(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    meta: &str,
    add_extra: impl FnOnce(&mut egui::Ui),
) {
    let fill = if selected {
        Color32::from_rgb(231, 242, 234)
    } else {
        Color32::from_rgb(248, 250, 247)
    };
    let stroke = if selected {
        Color32::from_rgb(109, 156, 121)
    } else {
        Color32::from_rgb(218, 226, 219)
    };

    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_width(ui.available_width());
            if !title.is_empty() || selected {
                ui.horizontal_wrapped(|ui| {
                    if !title.is_empty() {
                        ui.label(RichText::new(title).strong());
                    }
                    if selected && !title.is_empty() {
                        ui.small(RichText::new("已选中").color(Color32::from_rgb(72, 116, 84)));
                    }
                });
            }
            if !meta.is_empty() {
                ui.small(meta);
            }
            add_extra(ui);
        });
}

fn selection_toggle_with_badge(
    ui: &mut egui::Ui,
    selected: &mut bool,
    badge: (&'static str, Color32, Color32),
    label: &str,
) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        let response = ui.checkbox(selected, "");
        kind_badge(ui, badge);
        ui.label(RichText::new(label).size(15.5).strong());
        response
    })
    .inner
}

fn detail_line(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        RichText::new(text.into())
            .size(15.0)
            .color(Color32::from_rgb(68, 77, 88)),
    );
}

fn kind_badge(ui: &mut egui::Ui, badge: (&'static str, Color32, Color32)) {
    egui::Frame::new()
        .fill(badge.1)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(badge.0).size(12.0).strong().color(badge.2));
        });
}

fn path_kind_badge_from_path(path: Option<&Path>) -> (&'static str, Color32, Color32) {
    match path {
        Some(path) if path.is_dir() => (
            "文件夹",
            Color32::from_rgb(226, 240, 255),
            Color32::from_rgb(28, 84, 142),
        ),
        Some(path) if path.is_file() => (
            "文件",
            Color32::from_rgb(233, 247, 233),
            Color32::from_rgb(43, 112, 55),
        ),
        Some(_) => (
            "路径",
            Color32::from_rgb(239, 241, 245),
            Color32::from_rgb(83, 92, 104),
        ),
        None => (
            "记录",
            Color32::from_rgb(245, 238, 225),
            Color32::from_rgb(132, 92, 24),
        ),
    }
}

fn matches_filter(filter: &str, fields: &[&str]) -> bool {
    let query = filter.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }

    fields
        .iter()
        .any(|field| field.to_lowercase().contains(&query))
}

fn summarize_scan_user_roots(
    roots: &[crate::models::UserDataRoot],
    filter: &str,
    selected_user_roots: &HashSet<String>,
) -> ScanUserRootSummary {
    let mut summary = ScanUserRootSummary::default();

    for root in roots {
        if !matches_scan_user_root(root, filter) {
            continue;
        }

        let key = plan::path_key(&root.path);
        summary.filtered_count += 1;
        summary.visible_keys.push(key.clone());
        if selected_user_roots.contains(&key) {
            summary.visible_selected_count += 1;
        } else {
            summary.visible_unselected_count += 1;
        }
    }

    summary
}

fn matches_scan_user_root(root: &crate::models::UserDataRoot, filter: &str) -> bool {
    let path = root.path.display().to_string();
    matches_filter(filter, &[&root.label, &root.category, &root.reason, &path])
}

fn present_restore_error(message: &str) -> String {
    if message.contains("restore target already exists:") {
        "恢复中止：目标目录里已经有同名文件。可以改用新的恢复目录，或启用“跳过已存在文件”。"
            .to_string()
    } else if message.contains("Archive does not contain any files to restore.") {
        "当前恢复范围里没有可写出的文件。请先检查恢复范围选择。".to_string()
    } else if message.contains("restore destination is an existing file:") {
        "恢复目标无效：你选择的是一个文件，不是目录。请改成文件夹路径。".to_string()
    } else if message.contains("restore destination is blocked by existing file:")
        || message.contains("restore target is blocked by existing file:")
    {
        "恢复目标无效：目标路径中有某一层已经是文件，无法继续创建目录。请改用别的恢复位置。"
            .to_string()
    } else if message.contains("restore target path is duplicated in archive:") {
        "归档内容校验失败：存在重复的恢复目标路径，已阻止本次恢复。".to_string()
    } else if message.contains("failed to create restore destination") {
        "无法创建恢复目录。请检查目标路径是否可写。".to_string()
    } else if message.contains("escapes restore root") {
        "归档内容校验失败：发现了越界路径，WinRehome 已阻止这次恢复。".to_string()
    } else {
        message.to_string()
    }
}

fn present_backup_error(message: &str) -> String {
    if message.contains("backup output path is an existing file:") {
        "备份输出路径无效：你选中了一个文件，不是目录。请改成文件夹路径。".to_string()
    } else if message.contains("backup output path is blocked by existing file:") {
        "备份输出路径无效：目标路径中有某一层已经是文件，无法继续创建目录。".to_string()
    } else if message.contains("backup output path overlaps selected source directory:") {
        "备份输出路径无效：备份目录落在已选源目录里面，会和源数据混在一起。请换一个目录。"
            .to_string()
    } else if message.contains("duplicate archive entry path:") {
        "备份内容存在重复目标路径，已阻止生成归档。请调整选择范围后重试。".to_string()
    } else if message.contains("No files are selected for backup.") {
        "当前没有可打包的内容。请先选择至少一个用户目录或便携软件。".to_string()
    } else if message.contains("failed to create backup output directory") {
        "无法创建备份目录。请检查目标路径是否可写。".to_string()
    } else if message.contains("failed to create") {
        "无法写入备份归档。请检查目标目录是否可写，或确认同名文件没有被占用。".to_string()
    } else {
        message.to_string()
    }
}

fn preview_backup_output_directory(
    preview: &plan::BackupPreview,
    selected_user_roots: &HashSet<String>,
    selected_portable_apps: &HashSet<String>,
    selected_installed_app_dirs: &HashSet<String>,
    backup_output_input: &str,
) -> anyhow::Result<archive::BackupOutputPreflight> {
    let output_dir = if backup_output_input.trim().is_empty() {
        archive::default_output_dir()?
    } else {
        PathBuf::from(backup_output_input.trim())
    };
    archive::preview_backup_output(
        preview,
        selected_user_roots,
        selected_portable_apps,
        selected_installed_app_dirs,
        &output_dir,
    )
}

fn resolved_workspace(
    requested: WorkspaceView,
    _has_preview: bool,
    _has_loaded_archive: bool,
) -> WorkspaceView {
    requested
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= GB {
        format!("{:.2} GB", bytes_f / GB)
    } else if bytes_f >= MB {
        format!("{:.2} MB", bytes_f / MB)
    } else if bytes_f >= KB {
        format!("{:.2} KB", bytes_f / KB)
    } else {
        format!("{bytes} B")
    }
}

fn matches_restore_user_root(
    root: &archive::ManifestRoot,
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> bool {
    let key = user_restore_root_key(root);
    if only_unselected_roots && selected_restore_roots.contains(&key) {
        return false;
    }

    matches_filter(filter, &[&root.label, &root.category, &root.path])
}

fn matches_restore_portable_app(
    app: &archive::ManifestPortableApp,
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> bool {
    let key = portable_restore_root_key(app);
    if only_unselected_roots && selected_restore_roots.contains(&key) {
        return false;
    }

    matches_filter(
        filter,
        &[&app.display_name, &app.root_path, &app.main_executable],
    )
}

fn matches_restore_installed_app(
    app: &archive::ManifestInstalledApp,
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> bool {
    let Some(key) = installed_app_restore_root_key(app) else {
        return false;
    };
    if only_unselected_roots && selected_restore_roots.contains(&key) {
        return false;
    }

    matches_filter(
        filter,
        &[
            &app.display_name,
            &app.source,
            &app.install_location.clone().unwrap_or_default(),
        ],
    )
}

fn summarize_restore_user_roots(
    roots: &[archive::ManifestRoot],
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> RestoreRootListSummary {
    let mut summary = RestoreRootListSummary::default();

    for root in roots {
        if !matches_restore_user_root(root, filter, only_unselected_roots, selected_restore_roots) {
            continue;
        }

        let key = user_restore_root_key(root);
        summary.filtered_count += 1;
        summary.visible_keys.push(key.clone());
        if selected_restore_roots.contains(&key) {
            summary.visible_selected_count += 1;
        } else {
            summary.visible_unselected_count += 1;
        }
    }

    summary
}

fn summarize_restore_portable_apps(
    apps: &[archive::ManifestPortableApp],
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> RestoreRootListSummary {
    let mut summary = RestoreRootListSummary::default();

    for app in apps {
        if !matches_restore_portable_app(app, filter, only_unselected_roots, selected_restore_roots)
        {
            continue;
        }

        let key = portable_restore_root_key(app);
        summary.filtered_count += 1;
        summary.visible_keys.push(key.clone());
        if selected_restore_roots.contains(&key) {
            summary.visible_selected_count += 1;
        } else {
            summary.visible_unselected_count += 1;
        }
    }

    summary
}

fn summarize_restore_installed_app_dirs(
    apps: &[archive::ManifestInstalledApp],
    filter: &str,
    only_unselected_roots: bool,
    selected_restore_roots: &HashSet<String>,
) -> RestoreRootListSummary {
    let mut summary = RestoreRootListSummary::default();

    for app in apps {
        if !matches_restore_installed_app(
            app,
            filter,
            only_unselected_roots,
            selected_restore_roots,
        ) {
            continue;
        }

        let Some(key) = installed_app_restore_root_key(app) else {
            continue;
        };
        summary.filtered_count += 1;
        summary.visible_keys.push(key.clone());
        if selected_restore_roots.contains(&key) {
            summary.visible_selected_count += 1;
        } else {
            summary.visible_unselected_count += 1;
        }
    }

    summary
}

fn collect_restore_roots(loaded: &LoadedArchive) -> HashSet<String> {
    let mut roots = HashSet::new();
    for root in &loaded.manifest.selected_user_roots {
        roots.insert(user_restore_root_key(root));
    }
    for app in &loaded.manifest.selected_portable_apps {
        roots.insert(portable_restore_root_key(app));
    }
    for app in &loaded.manifest.installed_apps {
        if let Some(key) = installed_app_restore_root_key(app) {
            roots.insert(key);
        }
    }
    roots
}

fn user_restore_root_key(root: &archive::ManifestRoot) -> String {
    format!(
        "user/{}/{}",
        sanitize_restore_segment(&root.category),
        sanitize_restore_segment(&root.label)
    )
}

fn portable_restore_root_key(app: &archive::ManifestPortableApp) -> String {
    format!("portable/{}", sanitize_restore_segment(&app.display_name))
}

fn installed_app_restore_root_key(app: &archive::ManifestInstalledApp) -> Option<String> {
    app.backup_root
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn sanitize_restore_segment(value: &str) -> String {
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

fn retained_restore_roots(
    loaded: Option<&LoadedArchive>,
    saved_roots: &HashSet<String>,
) -> HashSet<String> {
    let Some(loaded) = loaded else {
        return HashSet::new();
    };
    let allowed_roots = collect_restore_roots(loaded);
    saved_roots
        .iter()
        .filter(|root| allowed_roots.contains(*root))
        .cloned()
        .collect()
}

fn restore_roots_for_loaded_archive(
    loaded: &LoadedArchive,
    previous_archive_path: Option<&Path>,
    previous_roots: &HashSet<String>,
) -> HashSet<String> {
    if previous_archive_path
        .map(|path| same_archive_path(path, &loaded.path))
        .unwrap_or(false)
    {
        return retained_restore_roots(Some(loaded), previous_roots);
    }

    collect_restore_roots(loaded)
}

fn restore_destination_for_loaded_archive(
    archive_path: &Path,
    same_archive_reload: bool,
    previous_destination: &str,
) -> String {
    if same_archive_reload && !previous_destination.trim().is_empty() {
        previous_destination.trim().to_string()
    } else {
        archive::default_restore_dir(archive_path)
            .map(|value| value.display().to_string())
            .unwrap_or_default()
    }
}

fn restore_text_filter_for_loaded_archive(
    same_archive_reload: bool,
    previous_filter: &str,
) -> String {
    if same_archive_reload {
        previous_filter.to_string()
    } else {
        String::new()
    }
}

fn restore_section_for_loaded_archive(
    same_archive_reload: bool,
    previous_section: RestoreSection,
) -> RestoreSection {
    if same_archive_reload {
        previous_section
    } else {
        RestoreSection::RestoreScope
    }
}

fn restore_flags_for_loaded_archive(
    same_archive_reload: bool,
    previous_restore_user_data: bool,
    previous_restore_portable_apps: bool,
    previous_restore_installed_app_dirs: bool,
    previous_skip_existing_restore_files: bool,
) -> (bool, bool, bool, bool) {
    if same_archive_reload {
        (
            previous_restore_user_data,
            previous_restore_portable_apps,
            previous_restore_installed_app_dirs,
            previous_skip_existing_restore_files,
        )
    } else {
        (true, true, true, false)
    }
}

fn same_archive_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn effective_restore_roots(
    loaded: &LoadedArchive,
    restore_user_data: bool,
    restore_portable_apps: bool,
    restore_installed_app_dirs: bool,
    selected_roots: &HashSet<String>,
) -> HashSet<String> {
    let available_roots = collect_restore_roots(loaded);
    selected_roots
        .iter()
        .filter(|root| {
            available_roots.contains(*root)
                && ((restore_user_data && root.starts_with("user/"))
                    || (restore_portable_apps && root.starts_with("portable/"))
                    || (restore_installed_app_dirs && root.starts_with("installed/")))
        })
        .cloned()
        .collect()
}

fn build_restore_preview_summary(
    loaded: &LoadedArchive,
    effective_roots: &HashSet<String>,
) -> RestorePreviewSummary {
    let mut summary = RestorePreviewSummary::default();

    for root in &loaded.manifest.selected_user_roots {
        if effective_roots.contains(&user_restore_root_key(root)) {
            summary.selected_user_root_count += 1;
        }
    }
    for app in &loaded.manifest.selected_portable_apps {
        if effective_roots.contains(&portable_restore_root_key(app)) {
            summary.selected_portable_app_count += 1;
        }
    }
    for app in &loaded.manifest.installed_apps {
        if let Some(key) = installed_app_restore_root_key(app) {
            if effective_roots.contains(&key) {
                summary.selected_installed_app_dir_count += 1;
            }
        }
    }

    summary.selected_root_count = summary.selected_user_root_count
        + summary.selected_portable_app_count
        + summary.selected_installed_app_dir_count;

    for entry in &loaded.manifest.files {
        if effective_roots.iter().any(|root| {
            entry.archive_path == *root || entry.archive_path.starts_with(&format!("{root}/"))
        }) {
            summary.selected_file_count += 1;
            summary.selected_bytes += entry.original_size;
        }
    }

    summary
}

#[cfg(test)]
fn portable_candidate_kind(root_path: &str, main_executable: &str) -> &'static str {
    if root_path.eq_ignore_ascii_case(main_executable) {
        "单文件可执行程序"
    } else {
        "目录型便携软件"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InstalledAppExportRow, LoadedArchive, build_restore_preview_summary, collapse_nested_paths,
        effective_restore_roots, escape_csv_field, export_installed_app_inventory_csv,
        matches_restore_portable_app, matches_restore_user_root, matches_scan_user_root,
        portable_candidate_kind, portable_restore_root_key, preview_backup_output_directory,
        restore_destination_for_loaded_archive, restore_flags_for_loaded_archive,
        restore_roots_for_loaded_archive, restore_section_for_loaded_archive,
        restore_text_filter_for_loaded_archive, summarize_restore_portable_apps,
        summarize_restore_user_roots, summarize_scan_user_roots, user_restore_root_key,
    };
    use crate::archive::{ArchiveManifest, ArchivedFileEntry, ManifestPortableApp, ManifestRoot};
    use crate::models::{InstalledAppRecord, PortableAppCandidate, PortableConfidence};
    use crate::models::{PathStats, UserDataRoot};
    use crate::plan::BackupPreview;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_loaded_archive() -> LoadedArchive {
        LoadedArchive {
            path: PathBuf::from("C:\\Backups\\sample.wrh"),
            manifest: ArchiveManifest {
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
                selected_portable_apps: vec![ManifestPortableApp {
                    display_name: "PortableTool".to_string(),
                    root_path: "D:\\PortableTool".to_string(),
                    main_executable: "D:\\PortableTool\\Tool.exe".to_string(),
                    confidence: "high".to_string(),
                    reasons: vec!["portable".to_string()],
                }],
                files: vec![
                    ArchivedFileEntry {
                        source_path: "a".to_string(),
                        archive_path: "user/Personal Files/Documents/note.txt".to_string(),
                        entry_kind: "user_data".to_string(),
                        offset: 0,
                        stored_size: 1,
                        original_size: 10,
                        crc32: 1,
                    },
                    ArchivedFileEntry {
                        source_path: "b".to_string(),
                        archive_path: "portable/PortableTool/Tool.exe".to_string(),
                        entry_kind: "portable_app".to_string(),
                        offset: 1,
                        stored_size: 1,
                        original_size: 20,
                        crc32: 2,
                    },
                ],
                original_bytes: 30,
                stored_bytes: 25,
            },
        }
    }

    fn sample_backup_preview() -> BackupPreview {
        BackupPreview {
            installed_apps: vec![InstalledAppRecord {
                display_name: "Git".to_string(),
                source: "hklm-64",
                install_location: Some(PathBuf::from("C:\\Program Files\\Git")),
                install_stats: Some(PathStats::default()),
                uninstall_key: "Git_is1".to_string(),
            }],
            portable_candidates: vec![PortableAppCandidate {
                display_name: "PortableTool".to_string(),
                root_path: PathBuf::from("D:\\PortableTool"),
                main_executable: PathBuf::from("D:\\PortableTool\\Tool.exe"),
                confidence: PortableConfidence::High,
                stats: PathStats::default(),
                reasons: Vec::new(),
            }],
            user_data_roots: vec![UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: PathBuf::from("C:\\Users\\Sunny\\Documents"),
                reason: "Test".into(),
                stats: PathStats::default(),
            }],
        }
    }

    #[test]
    fn restore_preview_summary_counts_only_selected_roots() {
        let loaded = sample_loaded_archive();
        let roots = HashSet::from([user_restore_root_key(
            &loaded.manifest.selected_user_roots[0],
        )]);

        let summary = build_restore_preview_summary(&loaded, &roots);

        assert_eq!(summary.selected_root_count, 1);
        assert_eq!(summary.selected_user_root_count, 1);
        assert_eq!(summary.selected_portable_app_count, 0);
        assert_eq!(summary.selected_file_count, 1);
        assert_eq!(summary.selected_bytes, 10);
    }

    #[test]
    fn effective_restore_roots_respects_category_toggles() {
        let loaded = sample_loaded_archive();
        let selected_roots = HashSet::from([
            user_restore_root_key(&loaded.manifest.selected_user_roots[0]),
            portable_restore_root_key(&loaded.manifest.selected_portable_apps[0]),
        ]);

        let effective = effective_restore_roots(&loaded, false, true, true, &selected_roots);

        assert_eq!(effective.len(), 1);
        assert!(effective.contains("portable/PortableTool"));
        assert!(!effective.contains("user/Personal Files/Documents"));
    }

    #[test]
    fn restore_scope_filter_can_hide_selected_roots() {
        let loaded = sample_loaded_archive();
        let user_root = &loaded.manifest.selected_user_roots[0];
        let portable_app = &loaded.manifest.selected_portable_apps[0];
        let selected = HashSet::from([
            user_restore_root_key(user_root),
            portable_restore_root_key(portable_app),
        ]);

        assert!(!matches_restore_user_root(user_root, "", true, &selected));
        assert!(!matches_restore_portable_app(
            portable_app,
            "",
            true,
            &selected
        ));
        assert!(matches_restore_user_root(user_root, "", false, &selected));
        assert!(matches_restore_portable_app(
            portable_app,
            "",
            false,
            &selected
        ));
    }

    #[test]
    fn restore_scope_summaries_count_visible_selected_and_unselected() {
        let loaded = sample_loaded_archive();
        let user_root = &loaded.manifest.selected_user_roots[0];
        let selected = HashSet::from([user_restore_root_key(user_root)]);

        let user_summary = summarize_restore_user_roots(
            &loaded.manifest.selected_user_roots,
            "",
            false,
            &selected,
        );
        let portable_summary = summarize_restore_portable_apps(
            &loaded.manifest.selected_portable_apps,
            "",
            false,
            &selected,
        );

        assert_eq!(user_summary.filtered_count, 1);
        assert_eq!(user_summary.visible_selected_count, 1);
        assert_eq!(user_summary.visible_unselected_count, 0);
        assert_eq!(portable_summary.filtered_count, 1);
        assert_eq!(portable_summary.visible_selected_count, 0);
        assert_eq!(portable_summary.visible_unselected_count, 1);
    }

    #[test]
    fn portable_candidate_kind_distinguishes_single_exe() {
        assert_eq!(
            portable_candidate_kind("D:\\Tools\\Tool.exe", "D:\\Tools\\Tool.exe"),
            "单文件可执行程序"
        );
        assert_eq!(
            portable_candidate_kind("D:\\Tools\\Tool", "D:\\Tools\\Tool\\Tool.exe"),
            "目录型便携软件"
        );
    }

    #[test]
    fn scan_user_root_filter_matches_text_fields() {
        let recommended = UserDataRoot {
            category: "Personal Files".into(),
            label: "Documents".into(),
            path: PathBuf::from("C:\\Users\\Sunny\\Documents"),
            reason: "Recommended".into(),
            stats: PathStats::default(),
        };

        assert!(matches_scan_user_root(&recommended, ""));
        assert!(matches_scan_user_root(&recommended, "documents"));
        assert!(matches_scan_user_root(&recommended, "personal files"));
        assert!(matches_scan_user_root(
            &recommended,
            "c:\\users\\sunny\\documents"
        ));
        assert!(!matches_scan_user_root(&recommended, "portable"));
    }

    #[test]
    fn scan_user_root_summary_counts_visible_selection_and_source() {
        let recommended_path = PathBuf::from("C:\\Users\\Sunny\\Documents");
        let custom_path = PathBuf::from("D:\\Configs\\portable.ini");
        let roots = vec![
            UserDataRoot {
                category: "Personal Files".into(),
                label: "Documents".into(),
                path: recommended_path.clone(),
                reason: "Recommended".into(),
                stats: PathStats::default(),
            },
            UserDataRoot {
                category: "Custom Files".into(),
                label: "Portable Config".into(),
                path: custom_path.clone(),
                reason: "User-added".into(),
                stats: PathStats::default(),
            },
        ];
        let selected = HashSet::from([crate::plan::path_key(&recommended_path)]);

        let summary = summarize_scan_user_roots(&roots, "", &selected);

        assert_eq!(summary.filtered_count, 2);
        assert_eq!(summary.visible_selected_count, 1);
        assert_eq!(summary.visible_unselected_count, 1);
        assert_eq!(summary.visible_keys.len(), 2);
        assert!(
            summary
                .visible_keys
                .contains(&crate::plan::path_key(&recommended_path))
        );
        assert!(
            summary
                .visible_keys
                .contains(&crate::plan::path_key(&custom_path))
        );
    }

    #[test]
    fn collapse_nested_paths_prefers_parent_directory() {
        let collapsed = collapse_nested_paths(vec![
            PathBuf::from("D:\\Tools\\PortableTool"),
            PathBuf::from("D:\\Tools"),
            PathBuf::from("D:\\Games"),
            PathBuf::from("D:\\Tools\\PortableTool\\Config"),
        ]);

        assert_eq!(collapsed.len(), 2);
        assert!(collapsed.contains(&PathBuf::from("D:\\Tools")));
        assert!(collapsed.contains(&PathBuf::from("D:\\Games")));
    }

    #[test]
    fn csv_field_escaping_wraps_quotes_and_commas() {
        assert_eq!(escape_csv_field("plain"), "plain");
        assert_eq!(escape_csv_field("with,comma"), "\"with,comma\"");
        assert_eq!(escape_csv_field("with\"quote"), "\"with\"\"quote\"");
    }

    #[test]
    fn installed_app_inventory_export_writes_csv() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let output_path =
            std::env::temp_dir().join(format!("winrehome-installed-apps-{unique}.csv"));
        let rows = vec![InstalledAppExportRow {
            display_name: "Tool, One".to_string(),
            source: "hkcu-64".to_string(),
            install_location: Some("C:\\Tools\\Tool One".to_string()),
            uninstall_key: "Tool\"One".to_string(),
        }];

        let count =
            export_installed_app_inventory_csv(&output_path, &rows).expect("export inventory");
        let csv = fs::read_to_string(&output_path).expect("read csv");

        assert_eq!(count, 1);
        assert!(csv.contains("DisplayName,Source,InstallLocation,RegistryKey"));
        assert!(csv.contains("\"Tool, One\""));
        assert!(csv.contains("\"Tool\"\"One\""));

        let _ = fs::remove_file(output_path);
    }

    #[test]
    fn reloading_same_archive_keeps_filtered_restore_roots() {
        let loaded = sample_loaded_archive();
        let selected = HashSet::from([portable_restore_root_key(
            &loaded.manifest.selected_portable_apps[0],
        )]);

        let retained = restore_roots_for_loaded_archive(&loaded, Some(&loaded.path), &selected);

        assert_eq!(retained, selected);
    }

    #[test]
    fn reloading_same_archive_keeps_empty_restore_selection() {
        let loaded = sample_loaded_archive();

        let retained =
            restore_roots_for_loaded_archive(&loaded, Some(&loaded.path), &HashSet::new());

        assert!(retained.is_empty());
    }

    #[test]
    fn loading_different_archive_resets_restore_selection_to_all_roots() {
        let loaded = sample_loaded_archive();
        let selected = HashSet::from([portable_restore_root_key(
            &loaded.manifest.selected_portable_apps[0],
        )]);

        let restored = restore_roots_for_loaded_archive(
            &loaded,
            Some(&PathBuf::from("D:\\Other\\different.wrh")),
            &selected,
        );

        assert_eq!(restored, super::collect_restore_roots(&loaded));
    }

    #[test]
    fn same_archive_reload_keeps_restore_destination_and_flags() {
        let archive_path = PathBuf::from("C:\\Backups\\sample.wrh");

        let destination =
            restore_destination_for_loaded_archive(&archive_path, true, "D:\\My Restore Target");
        let flags = restore_flags_for_loaded_archive(true, false, true, true, true);
        let section =
            restore_section_for_loaded_archive(true, super::RestoreSection::InstalledApps);
        let filter = restore_text_filter_for_loaded_archive(true, "portable");

        assert_eq!(destination, "D:\\My Restore Target");
        assert_eq!(flags, (false, true, true, true));
        assert_eq!(section, super::RestoreSection::InstalledApps);
        assert_eq!(filter, "portable");
    }

    #[test]
    fn different_archive_reload_resets_restore_runtime_state() {
        let archive_path = PathBuf::from("C:\\Backups\\sample.wrh");

        let destination =
            restore_destination_for_loaded_archive(&archive_path, false, "D:\\Old Restore");
        let flags = restore_flags_for_loaded_archive(false, false, false, true, true);
        let section =
            restore_section_for_loaded_archive(false, super::RestoreSection::InstalledApps);
        let filter = restore_text_filter_for_loaded_archive(false, "portable");

        assert!(destination.ends_with("WinRehome Restores\\sample"));
        assert_eq!(flags, (true, true, true, false));
        assert_eq!(section, super::RestoreSection::RestoreScope);
        assert!(filter.is_empty());
    }

    #[test]
    fn backup_preflight_accepts_missing_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("winrehome-missing-output-dir-{unique}"));
        let preview = sample_backup_preview();

        let preview = preview_backup_output_directory(
            &preview,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &path.display().to_string(),
        )
        .expect("missing output dir should be acceptable");

        assert_eq!(preview.output_dir, path);
        assert!(!preview.exists);
        assert!(!preview.is_directory);
    }

    #[test]
    fn backup_preflight_rejects_existing_file_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("winrehome-output-file-{unique}.txt"));
        fs::write(&path, b"file-not-dir").expect("write output file");
        let preview = sample_backup_preview();

        let error = preview_backup_output_directory(
            &preview,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &path.display().to_string(),
        )
        .expect_err("file output path should be rejected");

        assert!(error.to_string().contains("existing file"));

        let _ = fs::remove_file(path);
    }
}
