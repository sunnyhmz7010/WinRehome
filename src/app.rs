use crate::{archive, config, plan};
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RestorePreviewSummary {
    selected_root_count: usize,
    selected_user_root_count: usize,
    selected_portable_app_count: usize,
    selected_file_count: usize,
    selected_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WorkspaceView {
    #[default]
    Overview,
    ScanPlan,
    Restore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ScanPlanSection {
    InstalledApps,
    PortableApps,
    #[default]
    UserData,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RestoreSection {
    InstalledApps,
    #[default]
    RestoreScope,
    RestoreAction,
}

#[derive(Default)]
pub struct WinRehomeApp {
    active_workspace: WorkspaceView,
    scan_section: ScanPlanSection,
    restore_section: RestoreSection,
    preview: Option<plan::BackupPreview>,
    scan_filter: String,
    restore_filter: String,
    restore_inventory_filter: String,
    selected_user_roots: HashSet<String>,
    selected_portable_apps: HashSet<String>,
    backup_output_input: String,
    archive_path_input: String,
    restore_destination_input: String,
    restore_user_data: bool,
    restore_portable_apps: bool,
    selected_restore_roots: HashSet<String>,
    skip_existing_restore_files: bool,
    recent_archives: Vec<PathBuf>,
    loaded_archive: Option<LoadedArchive>,
    last_archive: Option<archive::BackupResult>,
    last_restore: Option<archive::RestoreResult>,
    last_verification: Option<archive::VerificationResult>,
    last_notice: Option<String>,
    last_error: Option<String>,
}

impl WinRehomeApp {
    pub fn new() -> Self {
        let mut app = Self {
            restore_user_data: true,
            restore_portable_apps: true,
            ..Self::default()
        };

        if let Ok(Some(saved)) = config::load_config() {
            let saved_restore_user_data = saved.restore_user_data;
            let saved_restore_portable_apps = saved.restore_portable_apps;
            let saved_skip_existing_restore_files = saved.skip_existing_restore_files;
            let saved_selected_restore_roots = saved.selected_restore_roots.clone();

            app.selected_user_roots = config::normalize_existing_paths(&saved.selected_user_roots);
            app.selected_portable_apps =
                config::normalize_existing_paths(&saved.selected_portable_apps);
            app.backup_output_input = saved.last_backup_output_dir.unwrap_or_default();
            app.archive_path_input = saved.last_archive_path.unwrap_or_default();
            app.restore_destination_input = saved.last_restore_destination.unwrap_or_default();
            app.restore_user_data = saved_restore_user_data;
            app.restore_portable_apps = saved_restore_portable_apps;
            app.selected_restore_roots = saved_selected_restore_roots.clone();
            app.skip_existing_restore_files = saved_skip_existing_restore_files;

            if !app.archive_path_input.trim().is_empty() {
                let saved_restore_destination = app.restore_destination_input.clone();
                let path = PathBuf::from(app.archive_path_input.trim());
                if path.exists() {
                    app.load_archive_from_path(path);
                    app.restore_user_data = saved_restore_user_data;
                    app.restore_portable_apps = saved_restore_portable_apps;
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
        let saved_user_roots: HashSet<String> = preview
            .user_data_roots
            .iter()
            .map(|root| plan::path_key(&root.path))
            .filter(|key| self.selected_user_roots.contains(key))
            .collect();
        let saved_portable_apps: HashSet<String> = preview
            .portable_candidates
            .iter()
            .map(|candidate| plan::path_key(&candidate.root_path))
            .filter(|key| self.selected_portable_apps.contains(key))
            .collect();

        self.selected_user_roots = if saved_user_roots.is_empty() {
            preview.default_user_root_keys()
        } else {
            saved_user_roots
        };
        self.selected_portable_apps = if saved_portable_apps.is_empty() {
            preview.default_portable_keys()
        } else {
            saved_portable_apps
        };
        self.preview = Some(preview);
        self.scan_filter.clear();
        self.scan_section = ScanPlanSection::UserData;
        self.active_workspace = WorkspaceView::ScanPlan;
        self.last_notice = None;
        self.last_error = None;
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
                let previous_restore_roots = self.selected_restore_roots.clone();
                self.restore_destination_input = archive::default_restore_dir(&path)
                    .map(|value| value.display().to_string())
                    .unwrap_or_default();
                self.archive_path_input = path.display().to_string();
                let loaded = LoadedArchive { path, manifest };
                self.selected_restore_roots = restore_roots_for_loaded_archive(
                    &loaded,
                    previous_archive_path.as_deref(),
                    &previous_restore_roots,
                );
                self.loaded_archive = Some(loaded);
                self.restore_filter.clear();
                self.restore_inventory_filter.clear();
                self.restore_section = RestoreSection::RestoreScope;
                self.active_workspace = WorkspaceView::Restore;
                self.restore_user_data = true;
                self.restore_portable_apps = true;
                self.skip_existing_restore_files = false;
                self.last_verification = None;
                self.last_restore = None;
                self.last_notice = None;
                self.last_error = None;
                self.refresh_recent_archives();
                let _ = self.persist_config();
            }
            Err(error) => {
                self.loaded_archive = None;
                self.last_verification = None;
                self.last_restore = None;
                self.last_error = Some(error.to_string());
            }
        }
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.scan_filter.clear();
        self.selected_user_roots.clear();
        self.selected_portable_apps.clear();
        self.active_workspace = if self.loaded_archive.is_some() {
            WorkspaceView::Restore
        } else {
            WorkspaceView::Overview
        };
        self.last_archive = None;
        self.last_verification = None;
        self.last_restore = None;
        self.last_notice = None;
        self.last_error = None;
    }

    fn persist_config(&self) -> anyhow::Result<PathBuf> {
        config::save_config(&config::AppConfig {
            selected_user_roots: self.selected_user_roots.clone(),
            selected_portable_apps: self.selected_portable_apps.clone(),
            last_backup_output_dir: (!self.backup_output_input.trim().is_empty())
                .then(|| self.backup_output_input.trim().to_string()),
            last_archive_path: (!self.archive_path_input.trim().is_empty())
                .then(|| self.archive_path_input.trim().to_string()),
            last_restore_destination: (!self.restore_destination_input.trim().is_empty())
                .then(|| self.restore_destination_input.trim().to_string()),
            restore_user_data: self.restore_user_data,
            restore_portable_apps: self.restore_portable_apps,
            selected_restore_roots: self.selected_restore_roots.clone(),
            skip_existing_restore_files: self.skip_existing_restore_files,
        })
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
    style.spacing.button_padding = egui::vec2(14.0, 10.0);
    style.spacing.menu_margin = egui::Margin::same(12);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.indent = 18.0;

    style.visuals = {
        let mut visuals = egui::Visuals::light();
        visuals.override_text_color = Some(Color32::from_rgb(28, 43, 38));
        visuals.panel_fill = Color32::from_rgb(244, 248, 244);
        visuals.extreme_bg_color = Color32::from_rgb(234, 241, 236);
        visuals.window_fill = Color32::from_rgb(252, 253, 250);
        visuals.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(202, 214, 205));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(252, 253, 250);
        visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(208, 219, 210));
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(18);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(245, 249, 246);
        visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(181, 202, 188));
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(14);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(232, 242, 235);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(92, 138, 110));
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(14);
        visuals.widgets.active.bg_fill = Color32::from_rgb(94, 138, 112);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(68, 107, 84));
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5, Color32::WHITE);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(14);
        visuals.selection.bg_fill = Color32::from_rgb(138, 176, 151);
        visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(58, 91, 72));
        visuals.hyperlink_color = Color32::from_rgb(62, 110, 148);
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
        egui::TopBottomPanel::top("hero_panel")
            .exact_height(196.0)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(224, 237, 227))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(175, 198, 181)))
                    .inner_margin(egui::Margin::symmetric(22, 16))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            hero_identity(
                                ui,
                                self.preview.is_some(),
                                self.last_archive.is_some(),
                                self.loaded_archive.is_some(),
                                !self.selected_user_roots.is_empty()
                                    || !self.selected_portable_apps.is_empty(),
                            );
                            ui.add_space(10.0);
                            ui.horizontal_wrapped(|ui| {
                                hero_primary_actions(self, ui);
                            });
                        });
                    });
            });

        egui::SidePanel::left("overview_panel")
            .resizable(false)
            .exact_width(236.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        card_panel(
                            ui,
                            "当前阶段",
                            "先扫描，再审查，再归档，最后恢复。",
                            |ui| {
                                ui.label(format!(
                                    "当前工作：{}",
                                    current_stage_label(
                                        self.preview.is_some(),
                                        self.loaded_archive.is_some()
                                    )
                                ));
                                if let Some(preview) = &self.preview {
                                    metric_tile(
                                        ui,
                                        Color32::from_rgb(231, 240, 233),
                                        "已发现候选",
                                        &format!(
                                            "{} 目录 / {} 便携 / {} 软件",
                                            preview.user_data_roots.len(),
                                            preview.portable_candidates.len(),
                                            preview.installed_apps.len()
                                        ),
                                    );
                                } else if let Some(loaded) = &self.loaded_archive {
                                    metric_tile(
                                        ui,
                                        Color32::from_rgb(233, 238, 245),
                                        "已加载归档",
                                        &format!(
                                            "{} 个文件 / {}",
                                            loaded.manifest.files.len(),
                                            format_bytes(loaded.manifest.stored_bytes)
                                        ),
                                    );
                                } else {
                                    ui.small("还没有扫描结果，也没有已加载的归档。");
                                }
                            },
                        );

                        if let Some(error) = &self.last_error {
                            status_banner(
                                ui,
                                Color32::from_rgb(252, 233, 229),
                                Color32::from_rgb(212, 122, 102),
                                &format!("操作失败：{error}"),
                            );
                        }
                        if let Some(notice) = &self.last_notice {
                            status_banner(
                                ui,
                                Color32::from_rgb(232, 239, 248),
                                Color32::from_rgb(108, 143, 184),
                                notice,
                            );
                        }
                        if let Some(result) = &self.last_archive {
                            status_banner(
                                ui,
                                Color32::from_rgb(230, 242, 233),
                                Color32::from_rgb(102, 150, 113),
                                &format!(
                                    "归档已创建：{}\n{} 个文件，原始大小 {}，归档大小 {}。",
                                    result.archive_path.display(),
                                    result.file_count,
                                    format_bytes(result.original_bytes),
                                    format_bytes(result.stored_bytes)
                                ),
                            );
                        }
                        if let Some(result) = &self.last_restore {
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
                        }
                        if let Some(result) = &self.last_verification {
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
                        }

                        card_panel(
                            ui,
                            "可靠性原则",
                            "默认宁可保守，也不把不该迁移的内容带进归档。",
                            |ui| {
                                principle_row(ui, "已安装软件只记录，不打包程序本体");
                                principle_row(ui, "便携软件先审查，再进入归档");
                                principle_row(ui, "个人文件按白名单和迁移价值选择");
                                principle_row(ui, "缓存、临时文件、日志默认排除");
                            },
                        );

                        if !self.recent_archives.is_empty() {
                            card_panel(
                                ui,
                                "最近归档",
                                "方便快速重新打开和验证。",
                                |ui| {
                                    for path in self.recent_archives.clone().into_iter().take(5) {
                                        let file_name = archive_file_label(&path);
                                        let archive_meta = recent_archive_meta(&path);
                                        result_card(ui, &file_name, &archive_meta, |ui| {
                                            ui.small(path.display().to_string());
                                            if ui
                                                .add(secondary_action_button("加载这个归档"))
                                                .clicked()
                                            {
                                                self.load_archive_from_path(path.clone());
                                            }
                                        });
                                        ui.add_space(6.0);
                                    }
                                },
                            );
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let resolved_workspace = resolved_workspace(
                        self.active_workspace,
                        self.preview.is_some(),
                        self.loaded_archive.is_some(),
                    );
                    workspace_switcher(
                        ui,
                        &mut self.active_workspace,
                        self.preview.is_some(),
                        self.loaded_archive.is_some(),
                    );
                    ui.add_space(10.0);

                    if matches!(resolved_workspace, WorkspaceView::Overview) {
                        card_panel(
                            ui,
                            "总览",
                            "WinRehome 不做整盘镜像，而是把真正值得迁移的内容整理成单文件归档。",
                            |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    metric_tile(
                                        ui,
                                        Color32::from_rgb(231, 240, 233),
                                        "步骤 1",
                                        "扫描已安装软件、便携候选和高价值用户目录",
                                    );
                                    metric_tile(
                                        ui,
                                        Color32::from_rgb(241, 237, 227),
                                        "步骤 2",
                                        "审查候选项，只保留真正值得迁移的内容",
                                    );
                                    metric_tile(
                                        ui,
                                        Color32::from_rgb(233, 238, 245),
                                        "步骤 3",
                                        "生成 `.wrh` 归档并在新系统中恢复",
                                    );
                                });

                                ui.add_space(12.0);
                                ui.columns(2, |columns| {
                                    quick_action_card(
                                        &mut columns[0],
                                        "开始新扫描",
                                        "重新评估当前机器上的个人目录、便携候选和已安装软件记录。",
                                        "进入扫描计划",
                                        || {
                                            match plan::build_preview() {
                                                Ok(preview) => self.load_preview(preview),
                                                Err(error) => {
                                                    self.last_archive = None;
                                                    self.last_error = Some(error.to_string());
                                                }
                                            }
                                        },
                                    );

                                    quick_action_card(
                                        &mut columns[1],
                                        "打开最近归档",
                                        "直接进入恢复工作区，查看内容、校验完整性并准备恢复。",
                                        "加载最新归档",
                                        || {
                                            self.refresh_recent_archives();
                                            match self.recent_archives.first().cloned() {
                                                Some(path) => self.load_archive_from_path(path),
                                                None => {
                                                    self.loaded_archive = None;
                                                    self.last_verification = None;
                                                    self.last_restore = None;
                                                    self.last_error = Some(
                                                        "没有在默认目录、当前备份目录或最近使用目录中找到 .wrh 归档。"
                                                            .to_string(),
                                                    );
                                                }
                                            }
                                        },
                                    );
                                });

                                if let Some(preview) = &self.preview {
                                    let scan_summary = preview.summarize_selection(
                                        &self.selected_user_roots,
                                        &self.selected_portable_apps,
                                    );
                                    ui.add_space(10.0);
                                    ui.label(RichText::new("最近一次扫描").strong());
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(239, 244, 231),
                                            "用户目录",
                                            &preview.user_data_roots.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(231, 240, 233),
                                            "便携候选",
                                            &preview.portable_candidates.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(233, 238, 245),
                                            "软件记录",
                                            &preview.installed_apps.len().to_string(),
                                        );
                                    });
                                    ui.add_space(8.0);
                                    card_panel(
                                        ui,
                                        "扫描结果已就绪",
                                        &format!(
                                            "当前已选 {} 个用户目录、{} 个便携软件，预计 {} 个文件。",
                                            scan_summary.selected_user_roots,
                                            scan_summary.selected_portable_apps,
                                            scan_summary.total_files
                                        ),
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if overview_jump_button(
                                                    ui,
                                                    "看用户目录",
                                                    "继续审查迁移价值最高的个人数据。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::ScanPlan;
                                                    self.scan_section = ScanPlanSection::UserData;
                                                }
                                                if overview_jump_button(
                                                    ui,
                                                    "看便携软件",
                                                    "确认哪些目录型或单文件程序应该一起带走。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::ScanPlan;
                                                    self.scan_section = ScanPlanSection::PortableApps;
                                                }
                                                if overview_jump_button(
                                                    ui,
                                                    "看软件记录",
                                                    "检查安装版软件清单，必要时导出 CSV。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::ScanPlan;
                                                    self.scan_section = ScanPlanSection::InstalledApps;
                                                }
                                            });
                                        },
                                    );
                                }
                                if let Some(loaded) = &self.loaded_archive {
                                    let loaded_archive_name = archive_file_label(&loaded.path);
                                    ui.add_space(10.0);
                                    ui.label(RichText::new("当前已加载归档").strong());
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(233, 238, 245),
                                            "归档文件数",
                                            &loaded.manifest.files.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 240, 233),
                                            "归档大小",
                                            &format_bytes(loaded.manifest.stored_bytes),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(240, 247, 241),
                                            "软件记录",
                                            &loaded.manifest.installed_apps.len().to_string(),
                                        );
                                    });
                                    ui.add_space(8.0);
                                    card_panel(
                                        ui,
                                        "恢复入口",
                                        &format!(
                                            "{} 已加载，可直接查看软件记录、选择恢复范围或进入执行恢复。",
                                            loaded_archive_name
                                        ),
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if overview_jump_button(
                                                    ui,
                                                    "看软件记录",
                                                    "对照归档里的安装版软件清单。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::Restore;
                                                    self.restore_section = RestoreSection::InstalledApps;
                                                }
                                                if overview_jump_button(
                                                    ui,
                                                    "选恢复范围",
                                                    "决定恢复哪些个人目录和便携软件。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::Restore;
                                                    self.restore_section = RestoreSection::RestoreScope;
                                                }
                                                if overview_jump_button(
                                                    ui,
                                                    "执行恢复",
                                                    "确认目标目录和策略后开始恢复。",
                                                ) {
                                                    self.active_workspace = WorkspaceView::Restore;
                                                    self.restore_section = RestoreSection::RestoreAction;
                                                }
                                            });
                                        },
                                    );
                                }
                            },
                        );
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
                            let visible_user_root_keys: Vec<String> = preview
                                .user_data_roots
                                .iter()
                                .filter_map(|root| {
                                    let path = root.path.display().to_string();
                                    matches_filter(
                                        &self.scan_filter,
                                        &[root.label, root.category, root.reason, &path],
                                    )
                                    .then(|| plan::path_key(&root.path))
                                })
                                .collect();
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
                            let filtered_user_root_count = preview
                                .user_data_roots
                                .iter()
                                .filter(|root| {
                                    let path = root.path.display().to_string();
                                    matches_filter(
                                        &self.scan_filter,
                                        &[root.label, root.category, root.reason, &path],
                                    )
                                })
                                .count();
                            let summary = preview.summarize_selection(
                                &self.selected_user_roots,
                                &self.selected_portable_apps,
                            );

                            card_panel(
                                ui,
                                "扫描计划",
                                "先筛选，再审查，最后输出到你指定的备份目录。",
                                |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(231, 240, 233),
                                            "软件记录",
                                            &preview.installed_apps.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(239, 244, 231),
                                            "便携候选",
                                            &preview.portable_candidates.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(233, 238, 245),
                                            "用户目录",
                                            &preview.user_data_roots.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 240, 233),
                                            "排除规则",
                                            &preview.exclusion_rules.len().to_string(),
                                        );
                                    });

                                    ui.add_space(10.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("统一筛选");
                                        ui.add(
                                            egui::TextEdit::singleline(&mut self.scan_filter)
                                                .desired_width(320.0)
                                                .hint_text("按软件名、目录名或路径过滤"),
                                        );
                                    });

                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("备份输出目录");
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(
                                                    &mut self.backup_output_input,
                                                )
                                                .desired_width(360.0)
                                                .hint_text("例如 D:\\WinRehome Backups"),
                                            )
                                            .changed()
                                        {
                                            let _ = self.persist_config();
                                            self.refresh_recent_archives();
                                        }
                                        if ui.add(secondary_action_button("浏览目录")).clicked() {
                                            if let Some(path) =
                                                pick_folder_from_input(&self.backup_output_input)
                                            {
                                                self.backup_output_input =
                                                    path.display().to_string();
                                                let _ = self.persist_config();
                                                self.refresh_recent_archives();
                                            }
                                        }
                                        if ui.add(secondary_action_button("默认目录")).clicked() {
                                            if let Ok(path) = archive::default_output_dir() {
                                                self.backup_output_input =
                                                    path.display().to_string();
                                                let _ = self.persist_config();
                                                self.refresh_recent_archives();
                                            }
                                        }
                                        if ui.add(secondary_action_button("打开目录")).clicked() {
                                            let path = PathBuf::from(self.backup_output_input.trim());
                                            if self.backup_output_input.trim().is_empty() {
                                                self.last_error = Some(
                                                    "请先填写或选择备份输出目录。".to_string(),
                                                );
                                                self.last_notice = None;
                                            } else if let Err(error) = open_path_in_explorer(&path) {
                                                self.last_error = Some(error.to_string());
                                                self.last_notice = None;
                                            }
                                        }
                                    });

                                    if !self.backup_output_input.trim().is_empty() {
                                        ui.small(format!(
                                            "归档文件会写入：{}",
                                            self.backup_output_input.trim()
                                        ));
                                    }

                                    ui.add_space(8.0);
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.add(secondary_action_button("使用推荐选择")).clicked()
                                        {
                                            self.selected_user_roots =
                                                preview.default_user_root_keys();
                                            self.selected_portable_apps =
                                                preview.default_portable_keys();
                                            let _ = self.persist_config();
                                        }

                                        if ui.add(secondary_action_button("清空选择")).clicked() {
                                            self.selected_user_roots.clear();
                                            self.selected_portable_apps.clear();
                                            let _ = self.persist_config();
                                        }

                                        if ui
                                            .add_enabled(
                                                summary.total_files > 0,
                                                primary_action_button("创建备份归档"),
                                            )
                                            .clicked()
                                        {
                                            let output_dir = if self
                                                .backup_output_input
                                                .trim()
                                                .is_empty()
                                            {
                                                archive::default_output_dir().map(|path| {
                                                    self.backup_output_input =
                                                        path.display().to_string();
                                                    let _ = self.persist_config();
                                                    path
                                                })
                                            } else {
                                                Ok(PathBuf::from(
                                                    self.backup_output_input.trim(),
                                                ))
                                            };

                                            match output_dir.and_then(|path| {
                                                let default_output_dir =
                                                    archive::default_output_dir().ok();
                                                if default_output_dir
                                                    .as_ref()
                                                    .is_some_and(|default_dir| default_dir == &path)
                                                {
                                                    archive::create_backup_archive(
                                                        &preview,
                                                        &self.selected_user_roots,
                                                        &self.selected_portable_apps,
                                                    )
                                                } else {
                                                    archive::create_backup_archive_in_dir(
                                                        &preview,
                                                        &self.selected_user_roots,
                                                        &self.selected_portable_apps,
                                                        &path,
                                                    )
                                                }
                                            }) {
                                                Ok(result) => {
                                                    self.load_archive_from_path(
                                                        result.archive_path.clone(),
                                                    );
                                                    self.last_archive = Some(result);
                                                    self.last_verification = None;
                                                    self.last_restore = None;
                                                    self.last_error = None;
                                                    let _ = self.persist_config();
                                                }
                                                Err(error) => {
                                                    self.last_archive = None;
                                                    self.loaded_archive = None;
                                                    self.last_restore = None;
                                                    self.last_error = Some(error.to_string());
                                                }
                                            }
                                        }
                                    });

                                    if !self.scan_filter.trim().is_empty()
                                        && filtered_installed_count == 0
                                        && filtered_portable_count == 0
                                        && filtered_user_root_count == 0
                                    {
                                        ui.add_space(8.0);
                                        compact_empty_state(
                                            ui,
                                            "没有匹配结果",
                                            "当前筛选条件下，没有命中的软件记录、便携候选或用户目录。",
                                        );
                                    }

                                    ui.add_space(8.0);
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(252, 248, 236),
                                            "已选用户目录",
                                            &summary.selected_user_roots.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(240, 247, 241),
                                            "已选便携软件",
                                            &summary.selected_portable_apps.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(238, 243, 249),
                                            "预计文件数",
                                            &summary.total_files.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 240, 233),
                                            "预计大小",
                                            &format_bytes(summary.total_bytes),
                                        );
                                    });
                                },
                            );

                            scan_plan_switcher(ui, &mut self.scan_section, &preview);

                            match self.scan_section {
                                ScanPlanSection::InstalledApps => {
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
                                    card_panel(
                                        ui,
                                        "已安装软件记录",
                                        "安装版软件只保留记录，不会把安装目录整体打包进归档。",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(
                                                    ui,
                                                    "筛选命中",
                                                    filtered_installed_count,
                                                );
                                                if ui
                                                    .add(secondary_action_button("导出命中 CSV"))
                                                    .clicked()
                                                {
                                                    let default_name = "WinRehome-installed-apps-scan.csv";
                                                    match pick_inventory_export_path(
                                                        default_name,
                                                        Some(self.backup_output_input.trim()),
                                                    )
                                                    {
                                                        Some(path) => {
                                                            match export_installed_app_inventory_csv(
                                                                &path,
                                                                &filtered_scan_apps,
                                                            ) {
                                                                Ok(count) => {
                                                                    self.last_notice = Some(format!(
                                                                        "软件记录已导出：{}，共 {} 条。",
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
                                                        None if filtered_scan_apps.is_empty() => {
                                                            self.last_error = Some(
                                                                "当前没有可导出的软件记录。".to_string(),
                                                            );
                                                            self.last_notice = None;
                                                        }
                                                        None => {}
                                                    }
                                                }
                                            });
                                            ui.add_space(8.0);
                                            if filtered_installed_count == 0 {
                                                compact_empty_state(
                                                    ui,
                                                    "没有命中的软件记录",
                                                    "可以调整筛选词，或切到其他分区继续审查。",
                                                );
                                            } else {
                                                egui::ScrollArea::vertical()
                                                    .max_height(460.0)
                                                    .show(ui, |ui| {
                                                        for app in
                                                            preview.installed_apps.iter().take(120)
                                                        {
                                                            let install_location = app
                                                                .install_location
                                                                .as_ref()
                                                                .map(|path| {
                                                                    path.display().to_string()
                                                                })
                                                                .unwrap_or_default();
                                                            if !matches_filter(
                                                                &self.scan_filter,
                                                                &[
                                                                    &app.display_name,
                                                                    app.source,
                                                                    &app.uninstall_key,
                                                                    &install_location,
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            installed_app_record_card(
                                                                ui,
                                                                &app.display_name,
                                                                app.source,
                                                                app.install_location
                                                                    .as_ref()
                                                                    .map(|path| {
                                                                        path.display()
                                                                            .to_string()
                                                                    }),
                                                                &app.uninstall_key,
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }
                                        },
                                    );
                                }
                                ScanPlanSection::PortableApps => {
                                    card_panel(
                                        ui,
                                        "便携软件候选",
                                        "这一栏只处理真正可随文件夹迁移的程序，包括目录型和单文件 EXE。",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(ui, "筛选命中", filtered_portable_count);
                                                if ui.add(secondary_action_button("全选命中")).clicked()
                                                {
                                                    for key in &visible_portable_keys {
                                                        self.selected_portable_apps
                                                            .insert(key.clone());
                                                    }
                                                    let _ = self.persist_config();
                                                }
                                                if ui.add(secondary_action_button("清空命中")).clicked()
                                                {
                                                    for key in &visible_portable_keys {
                                                        self.selected_portable_apps.remove(key);
                                                    }
                                                    let _ = self.persist_config();
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
                                                egui::ScrollArea::vertical()
                                                    .max_height(460.0)
                                                    .show(ui, |ui| {
                                                        for item in preview
                                                            .portable_candidates
                                                            .iter()
                                                            .take(60)
                                                        {
                                                            let root_path =
                                                                item.root_path.display().to_string();
                                                            let main_executable = item
                                                                .main_executable
                                                                .display()
                                                                .to_string();
                                                            if !matches_filter(
                                                                &self.scan_filter,
                                                                &[
                                                                    &item.display_name,
                                                                    &root_path,
                                                                    &main_executable,
                                                                    item.confidence_label(),
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            let key = plan::path_key(
                                                                &item.root_path,
                                                            );
                                                            let mut selected = self
                                                                .selected_portable_apps
                                                                .contains(&key);
                                                            if ui
                                                                .checkbox(
                                                                    &mut selected,
                                                                    &item.display_name,
                                                                )
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    self.selected_portable_apps
                                                                        .insert(key.clone());
                                                                } else {
                                                                    self.selected_portable_apps
                                                                        .remove(&key);
                                                                }
                                                                let _ = self.persist_config();
                                                            }
                                                            selection_result_card(
                                                                ui,
                                                                selected,
                                                                &item.display_name,
                                                                "便携软件候选",
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "类型：{} | 置信度：{}",
                                                                        portable_candidate_kind(
                                                                            &root_path,
                                                                            &main_executable,
                                                                        ),
                                                                        item.confidence_label()
                                                                    ));
                                                                    ui.small(format!(
                                                                        "来源路径：{}",
                                                                        root_path
                                                                    ));
                                                                    ui.small(format!(
                                                                        "主程序：{}",
                                                                        main_executable
                                                                    ));
                                                                    ui.small(format!(
                                                                        "预计大小：{}，共 {} 个文件",
                                                                        format_bytes(
                                                                            item.stats.total_bytes,
                                                                        ),
                                                                        item.stats.file_count
                                                                    ));
                                                                    for reason in item
                                                                        .reasons
                                                                        .iter()
                                                                        .take(3)
                                                                    {
                                                                        ui.small(reason);
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
                                ScanPlanSection::UserData => {
                                    card_panel(
                                        ui,
                                        "用户数据目录",
                                        "优先保留真正有迁移价值的个人文件与配置，而不是把整块系统噪音一起带走。",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(ui, "筛选命中", filtered_user_root_count);
                                                if ui.add(secondary_action_button("全选命中")).clicked()
                                                {
                                                    for key in &visible_user_root_keys {
                                                        self.selected_user_roots
                                                            .insert(key.clone());
                                                    }
                                                    let _ = self.persist_config();
                                                }
                                                if ui.add(secondary_action_button("清空命中")).clicked()
                                                {
                                                    for key in &visible_user_root_keys {
                                                        self.selected_user_roots.remove(key);
                                                    }
                                                    let _ = self.persist_config();
                                                }
                                            });
                                            ui.add_space(8.0);
                                            if filtered_user_root_count == 0 {
                                                compact_empty_state(
                                                    ui,
                                                    "没有命中的用户目录",
                                                    "当前筛选词没有匹配到目录或配置项。",
                                                );
                                            } else {
                                                egui::ScrollArea::vertical()
                                                    .max_height(420.0)
                                                    .show(ui, |ui| {
                                                        for root in &preview.user_data_roots {
                                                            let path =
                                                                root.path.display().to_string();
                                                            if !matches_filter(
                                                                &self.scan_filter,
                                                                &[
                                                                    root.label,
                                                                    root.category,
                                                                    root.reason,
                                                                    &path,
                                                                ],
                                                            ) {
                                                                continue;
                                                            }
                                                            let key = plan::path_key(&root.path);
                                                            let mut selected = self
                                                                .selected_user_roots
                                                                .contains(&key);
                                                            if ui
                                                                .checkbox(&mut selected, root.label)
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    self.selected_user_roots
                                                                        .insert(key.clone());
                                                                } else {
                                                                    self.selected_user_roots
                                                                        .remove(&key);
                                                                }
                                                                let _ = self.persist_config();
                                                            }
                                                            selection_result_card(
                                                                ui,
                                                                selected,
                                                                root.label,
                                                                root.category,
                                                                |ui| {
                                                                    ui.small(format!(
                                                                        "路径：{}",
                                                                        path
                                                                    ));
                                                                    ui.small(root.reason);
                                                                    ui.small(format!(
                                                                        "预计大小：{}，共 {} 个文件",
                                                                        format_bytes(
                                                                            root.stats.total_bytes,
                                                                        ),
                                                                        root.stats.file_count
                                                                    ));
                                                                },
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }
                                        },
                                    );

                                    card_panel(
                                        ui,
                                        "默认排除规则",
                                        "缓存、临时文件和构建产物不会默认进入迁移归档。",
                                        |ui| {
                                            for rule in &preview.exclusion_rules {
                                                principle_row(
                                                    ui,
                                                    &format!("{}: {}", rule.label, rule.pattern),
                                                );
                                            }
                                        },
                                    );
                                }
                            }
                        } else {
                            card_panel(
                                ui,
                                "还没有扫描计划",
                                "先运行一次扫描，WinRehome 才能列出可迁移的用户目录和便携软件候选。",
                                |ui| {
                                    ui.small("点击顶部的“生成扫描预览”后，这里会出现可审查的备份计划。");
                                },
                            );
                        }
                    }

                    if matches!(resolved_workspace, WorkspaceView::Restore) {
                        if let Some(loaded) = self.loaded_archive.clone() {
                            let visible_restore_user_keys: Vec<String> = loaded
                                .manifest
                                .selected_user_roots
                                .iter()
                                .filter_map(|root| {
                                    matches_filter(
                                        &self.restore_filter,
                                        &[&root.label, &root.category, &root.path],
                                    )
                                    .then(|| user_restore_root_key(root))
                                })
                                .collect();
                            let visible_restore_portable_keys: Vec<String> = loaded
                                .manifest
                                .selected_portable_apps
                                .iter()
                                .filter_map(|app| {
                                    matches_filter(
                                        &self.restore_filter,
                                        &[&app.display_name, &app.root_path, &app.main_executable],
                                    )
                                    .then(|| portable_restore_root_key(app))
                                })
                                .collect();
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
                            let all_restore_roots = collect_restore_roots(&loaded);
                        let effective_restore_roots = effective_restore_roots(
                            &loaded,
                            self.restore_user_data,
                            self.restore_portable_apps,
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
                                    selected_roots: effective_restore_roots.clone(),
                                    skip_existing_files: self.skip_existing_restore_files,
                                },
                            ))
                        };

                            card_panel(
                                ui,
                                "归档恢复",
                                "把“查看软件记录”“选择恢复范围”“执行恢复”拆开，避免所有控件挤在同一屏。",
                                |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(233, 238, 245),
                                            "归档大小",
                                            &format_bytes(loaded.manifest.stored_bytes),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(241, 247, 241),
                                            "软件记录",
                                            &loaded.manifest.installed_apps.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(252, 248, 236),
                                            "用户目录",
                                            &loaded.manifest.selected_user_roots.len().to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 240, 233),
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
                                        ui.label("归档文件");
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
                                            Color32::from_rgb(252, 248, 236),
                                            "已选个人目录",
                                            &restore_summary.selected_user_root_count.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(240, 247, 241),
                                            "已选便携软件",
                                            &restore_summary
                                                .selected_portable_app_count
                                                .to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(238, 243, 249),
                                            "预计恢复文件",
                                            &restore_summary.selected_file_count.to_string(),
                                        );
                                        metric_tile(
                                            ui,
                                            Color32::from_rgb(245, 240, 233),
                                            "预计恢复大小",
                                            &format_bytes(restore_summary.selected_bytes),
                                        );
                                    });
                                },
                            );

                            restore_section_switcher(ui, &mut self.restore_section, &loaded);

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
                                        "已安装软件记录",
                                        "这些项目只作为重装参考清单，不会从归档里恢复程序本体。",
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("软件记录筛选");
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.restore_inventory_filter,
                                                    )
                                                    .desired_width(320.0)
                                                    .hint_text("按软件名、来源或安装路径过滤"),
                                                );
                                            });
                                            ui.add_space(8.0);
                                            ui.horizontal_wrapped(|ui| {
                                                section_counter(
                                                    ui,
                                                    "筛选命中",
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
                                            if filtered_restore_installed_count == 0 {
                                                compact_empty_state(
                                                    ui,
                                                    "没有命中的软件记录",
                                                    "调整筛选词后，可以在这里查看安装版软件清单。",
                                                );
                                            } else {
                                                egui::ScrollArea::vertical()
                                                    .max_height(460.0)
                                                    .show(ui, |ui| {
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
                                                            installed_app_record_card(
                                                                ui,
                                                                &app.display_name,
                                                                &app.source,
                                                                app.install_location.clone(),
                                                                &app.uninstall_key,
                                                            );
                                                            ui.add_space(6.0);
                                                        }
                                                    });
                                            }
                                        },
                                    );
                                }
                                RestoreSection::RestoreScope => {
                                    card_panel(
                                        ui,
                                        "恢复范围",
                                        "先决定恢复哪些类别和哪些具体根目录，再进入执行恢复。",
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

                                            if !self.restore_filter.trim().is_empty()
                                                && filtered_restore_user_count == 0
                                                && filtered_restore_portable_count == 0
                                            {
                                                ui.add_space(8.0);
                                                compact_empty_state(
                                                    ui,
                                                    "没有命中的恢复范围",
                                                    "当前筛选词没有匹配到任何用户目录或便携软件。",
                                                );
                                            }

                                            if !loaded.manifest.selected_user_roots.is_empty() {
                                                ui.add_space(10.0);
                                                ui.label(RichText::new("个人文件目录").strong());
                                                ui.horizontal_wrapped(|ui| {
                                                    section_counter(
                                                        ui,
                                                        "筛选命中",
                                                        filtered_restore_user_count,
                                                    );
                                                    if ui.add(secondary_action_button("全选命中")).clicked()
                                                    {
                                                        for key in &visible_restore_user_keys {
                                                            self.selected_restore_roots
                                                                .insert(key.clone());
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                    if ui.add(secondary_action_button("清空命中")).clicked()
                                                    {
                                                        for key in &visible_restore_user_keys {
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
                                                        filtered_restore_portable_count,
                                                    );
                                                    if ui.add(secondary_action_button("全选命中")).clicked()
                                                    {
                                                        for key in
                                                            &visible_restore_portable_keys
                                                        {
                                                            self.selected_restore_roots
                                                                .insert(key.clone());
                                                        }
                                                        let _ = self.persist_config();
                                                    }
                                                    if ui.add(secondary_action_button("清空命中")).clicked()
                                                    {
                                                        for key in
                                                            &visible_restore_portable_keys
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
                                        },
                                    );
                                }
                                RestoreSection::RestoreAction => {
                                    card_panel(
                                        ui,
                                        "执行恢复",
                                        "最后确认目标目录和安全策略，然后开始恢复。",
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
                                                    && restore_summary.selected_file_count > 0;
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
                                                    let use_default_restore = self
                                                        .restore_user_data
                                                        && self.restore_portable_apps
                                                        && !self.skip_existing_restore_files
                                                        && self.selected_restore_roots
                                                            == collect_restore_roots(&loaded);
                                                    let restore_result = if use_default_restore {
                                                        archive::restore_archive(
                                                            &loaded.path,
                                                            &destination,
                                                        )
                                                    } else {
                                                        archive::restore_archive_with_selection(
                                                            &loaded.path,
                                                            &destination,
                                                            archive::RestoreSelection {
                                                                restore_user_data:
                                                                    self.restore_user_data,
                                                                restore_portable_apps: self
                                                                    .restore_portable_apps,
                                                                selected_roots:
                                                                    effective_restore_roots.clone(),
                                                                skip_existing_files: self
                                                                    .skip_existing_restore_files,
                                                            },
                                                        )
                                                    };

                                                    match restore_result {
                                                        Ok(result) => {
                                                            self.last_restore = Some(result);
                                                            self.last_verification = None;
                                                            self.last_notice = None;
                                                            self.last_error = None;
                                                            let _ = self.persist_config();
                                                        }
                                                        Err(error) => {
                                                            self.last_restore = None;
                                                            self.last_error = Some(
                                                                present_restore_error(
                                                                    &error.to_string(),
                                                                ),
                                                            );
                                                            self.last_notice = None;
                                                        }
                                                    }
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
                            card_panel(
                                ui,
                                "还没有已加载归档",
                                "先加载一个 `.wrh` 归档，才能查看内容、校验完整性并执行恢复。",
                                |ui| {
                                    ui.small("可以使用顶部的“加载最新归档”，也可以手动选择归档文件。");
                                },
                            );
                        }
                    }
                });
        });
    }
}

fn card_panel(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(252, 253, 250))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(205, 216, 208)))
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .size(19.0)
                    .strong()
                    .color(Color32::from_rgb(27, 47, 37)),
            );
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).color(Color32::from_rgb(92, 109, 99)));
                ui.add_space(8.0);
            }
            add_contents(ui);
        });
}

fn hero_identity(
    ui: &mut egui::Ui,
    has_preview: bool,
    has_archive: bool,
    has_loaded_archive: bool,
    has_review_selection: bool,
) {
    ui.label(
        RichText::new("WinRehome")
            .size(26.0)
            .strong()
            .color(Color32::from_rgb(24, 45, 36)),
    );
    ui.label(
        RichText::new("一个尽量节省空间的 Windows 迁移备份工具，只保留真正值得迁移的数据。")
            .color(Color32::from_rgb(72, 96, 82)),
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        flow_chip(ui, "扫描", has_preview);
        flow_chip(ui, "审查", has_preview && has_review_selection);
        flow_chip(ui, "归档", has_archive);
        flow_chip(ui, "恢复", has_loaded_archive);
    });
    ui.small(format!(
        "当前阶段：{}",
        current_stage_label(has_preview, has_loaded_archive)
    ));
}

fn hero_primary_actions(app: &mut WinRehomeApp, ui: &mut egui::Ui) {
    if ui.add(secondary_action_button("加载最新归档")).clicked() {
        app.refresh_recent_archives();
        match app.recent_archives.first().cloned() {
            Some(path) => app.load_archive_from_path(path),
            None => {
                app.loaded_archive = None;
                app.last_verification = None;
                app.last_restore = None;
                app.last_error = Some(
                    "没有在默认目录、当前备份目录或最近使用目录中找到 .wrh 归档。".to_string(),
                );
            }
        }
    }

    if ui.add(secondary_action_button("清空选择")).clicked() {
        app.clear_preview();
    }

    if ui.add(primary_action_button("生成扫描预览")).clicked() {
        match plan::build_preview() {
            Ok(preview) => app.load_preview(preview),
            Err(error) => {
                app.last_archive = None;
                app.last_error = Some(error.to_string());
            }
        }
    }
}

fn scan_plan_switcher(
    ui: &mut egui::Ui,
    active_section: &mut ScanPlanSection,
    preview: &plan::BackupPreview,
) {
    card_panel(
        ui,
        "扫描分区",
        "逐个分区审查，避免三栏同时滚动带来的干扰。",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if segment_button(
                    ui,
                    *active_section == ScanPlanSection::UserData,
                    &format!("用户目录 {}", preview.user_data_roots.len()),
                ) {
                    *active_section = ScanPlanSection::UserData;
                }
                if segment_button(
                    ui,
                    *active_section == ScanPlanSection::PortableApps,
                    &format!("便携候选 {}", preview.portable_candidates.len()),
                ) {
                    *active_section = ScanPlanSection::PortableApps;
                }
                if segment_button(
                    ui,
                    *active_section == ScanPlanSection::InstalledApps,
                    &format!("软件记录 {}", preview.installed_apps.len()),
                ) {
                    *active_section = ScanPlanSection::InstalledApps;
                }
            });
        },
    );
}

fn restore_section_switcher(
    ui: &mut egui::Ui,
    active_section: &mut RestoreSection,
    loaded: &LoadedArchive,
) {
    card_panel(
        ui,
        "恢复分区",
        "先看软件记录，再选恢复范围，最后执行恢复。",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if segment_button(
                    ui,
                    *active_section == RestoreSection::InstalledApps,
                    &format!("软件记录 {}", loaded.manifest.installed_apps.len()),
                ) {
                    *active_section = RestoreSection::InstalledApps;
                }
                if segment_button(
                    ui,
                    *active_section == RestoreSection::RestoreScope,
                    &format!(
                        "恢复范围 {}",
                        loaded.manifest.selected_user_roots.len()
                            + loaded.manifest.selected_portable_apps.len()
                    ),
                ) {
                    *active_section = RestoreSection::RestoreScope;
                }
                if segment_button(
                    ui,
                    *active_section == RestoreSection::RestoreAction,
                    "执行恢复",
                ) {
                    *active_section = RestoreSection::RestoreAction;
                }
            });
        },
    );
}

fn segment_button(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).strong().color(if selected {
            Color32::WHITE
        } else {
            Color32::from_rgb(40, 58, 48)
        }))
        .fill(if selected {
            Color32::from_rgb(87, 130, 102)
        } else {
            Color32::from_rgb(240, 245, 241)
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                Color32::from_rgb(72, 114, 87)
            } else {
                Color32::from_rgb(201, 213, 205)
            },
        ))
        .corner_radius(egui::CornerRadius::same(18))
        .min_size(egui::vec2(108.0, 34.0)),
    )
    .clicked()
}

fn installed_app_record_card(
    ui: &mut egui::Ui,
    display_name: &str,
    source: &str,
    install_location: Option<String>,
    uninstall_key: &str,
) {
    result_card(ui, display_name, &format!("来源：{source}"), |ui| {
        if let Some(path) = install_location.filter(|value| !value.trim().is_empty()) {
            ui.small(format!("安装位置：{path}"));
        }
        ui.small(format!("注册表键：{uninstall_key}"));
    });
}

fn overview_jump_button(ui: &mut egui::Ui, title: &str, description: &str) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(Color32::from_rgb(246, 249, 246))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(211, 221, 213)))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.small(description);
            ui.add_space(6.0);
            if ui.add(primary_action_button("进入")).clicked() {
                clicked = true;
            }
        });
    clicked
}

fn archive_file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("未知归档")
        .to_string()
}

fn recent_archive_meta(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => format!("{} | .wrh 归档", format_bytes(metadata.len())),
        Err(_) => ".wrh 归档".to_string(),
    }
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

fn workspace_switcher(
    ui: &mut egui::Ui,
    active_workspace: &mut WorkspaceView,
    has_preview: bool,
    has_loaded_archive: bool,
) {
    card_panel(
        ui,
        "工作区",
        "把扫描计划和归档恢复分开，避免所有信息挤在同一屏。",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                workspace_button(ui, active_workspace, WorkspaceView::Overview, "总览", true);
                workspace_button(
                    ui,
                    active_workspace,
                    WorkspaceView::ScanPlan,
                    "扫描计划",
                    has_preview,
                );
                workspace_button(
                    ui,
                    active_workspace,
                    WorkspaceView::Restore,
                    "归档恢复",
                    has_loaded_archive,
                );
            });
        },
    );
}

fn workspace_button(
    ui: &mut egui::Ui,
    active_workspace: &mut WorkspaceView,
    target: WorkspaceView,
    label: &str,
    enabled: bool,
) {
    let selected = *active_workspace == target;
    let button = egui::Button::new(RichText::new(label).strong().color(if selected {
        Color32::WHITE
    } else {
        Color32::from_rgb(42, 60, 49)
    }))
    .fill(if selected {
        Color32::from_rgb(87, 130, 102)
    } else {
        Color32::from_rgb(239, 244, 239)
    });

    if ui.add_enabled(enabled, button).clicked() {
        *active_workspace = target;
    }
}

fn flow_chip(ui: &mut egui::Ui, label: &str, active: bool) {
    let fill = if active {
        Color32::from_rgb(94, 138, 112)
    } else {
        Color32::from_rgb(238, 244, 239)
    };
    let text = if active {
        Color32::WHITE
    } else {
        Color32::from_rgb(76, 96, 84)
    };

    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(
            1.0,
            if active {
                Color32::from_rgb(72, 114, 87)
            } else {
                Color32::from_rgb(205, 216, 208)
            },
        ))
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.label(RichText::new(label).strong().color(text));
        });
}

fn primary_action_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
        .fill(Color32::from_rgb(87, 130, 102))
        .min_size(egui::vec2(120.0, 34.0))
}

fn secondary_action_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::from_rgb(43, 60, 49)))
        .fill(Color32::from_rgb(241, 245, 241))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(196, 210, 201)))
        .min_size(egui::vec2(108.0, 34.0))
}

fn quick_action_card(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    button_label: &str,
    action: impl FnOnce(),
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(246, 249, 246))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(211, 221, 213)))
        .corner_radius(egui::CornerRadius::same(16))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.small(description);
            ui.add_space(8.0);
            if ui.add(primary_action_button(button_label)).clicked() {
                action();
            }
        });
}

fn metric_tile(ui: &mut egui::Ui, fill: Color32, label: &str, value: &str) {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(205, 216, 208)))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .size(12.0)
                    .color(Color32::from_rgb(86, 103, 93)),
            );
            ui.label(
                RichText::new(value)
                    .size(18.0)
                    .strong()
                    .color(Color32::from_rgb(30, 49, 39)),
            );
        });
}

fn status_banner(ui: &mut egui::Ui, fill: Color32, stroke: Color32, text: &str) {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(Color32::from_rgb(44, 55, 49)));
        });
}

fn principle_row(ui: &mut egui::Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("•").color(Color32::from_rgb(95, 138, 109)));
        ui.small(text);
    });
}

fn compact_empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(247, 249, 246))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(218, 226, 219)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(58, 77, 66)),
            );
            ui.small(body);
        });
}

fn section_counter(ui: &mut egui::Ui, label: &str, count: usize) {
    egui::Frame::new()
        .fill(Color32::from_rgb(241, 246, 242))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(205, 216, 208)))
        .corner_radius(egui::CornerRadius::same(24))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small(RichText::new(label).color(Color32::from_rgb(84, 100, 90)));
                ui.label(
                    RichText::new(count.to_string())
                        .strong()
                        .color(Color32::from_rgb(33, 53, 42)),
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
            ui.label(RichText::new(title).strong());
            ui.small(meta);
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
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(title).strong());
                if selected {
                    ui.small(RichText::new("已选中").color(Color32::from_rgb(72, 116, 84)));
                }
            });
            ui.small(meta);
            add_extra(ui);
        });
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

fn current_stage_label(has_preview: bool, has_loaded_archive: bool) -> &'static str {
    match (has_preview, has_loaded_archive) {
        (true, true) => "扫描结果和归档恢复都已就绪",
        (true, false) => "正在审查扫描结果",
        (false, true) => "正在准备恢复已有归档",
        (false, false) => "等待开始第一次扫描",
    }
}

fn present_restore_error(message: &str) -> String {
    if message.contains("restore target already exists:") {
        "恢复中止：目标目录里已经有同名文件。可以改用新的恢复目录，或启用“跳过已存在文件”。"
            .to_string()
    } else if message.contains("Archive does not contain any files to restore.") {
        "当前恢复范围里没有可写出的文件。请先检查恢复范围选择。".to_string()
    } else if message.contains("restore destination is an existing file:") {
        "恢复目标无效：你选择的是一个文件，不是目录。请改成文件夹路径。".to_string()
    } else if message.contains("failed to create restore destination") {
        "无法创建恢复目录。请检查目标路径是否可写。".to_string()
    } else if message.contains("escapes restore root") {
        "归档内容校验失败：发现了越界路径，WinRehome 已阻止这次恢复。".to_string()
    } else {
        message.to_string()
    }
}

fn resolved_workspace(
    requested: WorkspaceView,
    has_preview: bool,
    has_loaded_archive: bool,
) -> WorkspaceView {
    match requested {
        WorkspaceView::Overview => WorkspaceView::Overview,
        WorkspaceView::ScanPlan if has_preview => WorkspaceView::ScanPlan,
        WorkspaceView::Restore if has_loaded_archive => WorkspaceView::Restore,
        _ if has_preview => WorkspaceView::ScanPlan,
        _ if has_loaded_archive => WorkspaceView::Restore,
        _ => WorkspaceView::Overview,
    }
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

fn collect_restore_roots(loaded: &LoadedArchive) -> HashSet<String> {
    let mut roots = HashSet::new();
    for root in &loaded.manifest.selected_user_roots {
        roots.insert(user_restore_root_key(root));
    }
    for app in &loaded.manifest.selected_portable_apps {
        roots.insert(portable_restore_root_key(app));
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

fn same_archive_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn effective_restore_roots(
    loaded: &LoadedArchive,
    restore_user_data: bool,
    restore_portable_apps: bool,
    selected_roots: &HashSet<String>,
) -> HashSet<String> {
    let available_roots = collect_restore_roots(loaded);
    selected_roots
        .iter()
        .filter(|root| {
            available_roots.contains(*root)
                && ((restore_user_data && root.starts_with("user/"))
                    || (restore_portable_apps && root.starts_with("portable/")))
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

    summary.selected_root_count =
        summary.selected_user_root_count + summary.selected_portable_app_count;

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
        InstalledAppExportRow, LoadedArchive, build_restore_preview_summary,
        effective_restore_roots, escape_csv_field, export_installed_app_inventory_csv,
        portable_candidate_kind, portable_restore_root_key, restore_roots_for_loaded_archive,
        user_restore_root_key,
    };
    use crate::archive::{ArchiveManifest, ArchivedFileEntry, ManifestPortableApp, ManifestRoot};
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

        let effective = effective_restore_roots(&loaded, false, true, &selected_roots);

        assert_eq!(effective.len(), 1);
        assert!(effective.contains("portable/PortableTool"));
        assert!(!effective.contains("user/Personal Files/Documents"));
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
}
