use crate::{archive, config, plan};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct LoadedArchive {
    path: PathBuf,
    manifest: archive::ArchiveManifest,
}

#[derive(Default)]
pub struct WinRehomeApp {
    preview: Option<plan::BackupPreview>,
    selected_user_roots: HashSet<String>,
    selected_portable_apps: HashSet<String>,
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
        app.recent_archives = archive::list_recent_archives(8).unwrap_or_default();
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
        self.last_error = None;
    }

    fn load_archive_from_path(&mut self, path: PathBuf) {
        match archive::read_archive_manifest(&path) {
            Ok(manifest) => {
                self.restore_destination_input = archive::default_restore_dir(&path)
                    .map(|value| value.display().to_string())
                    .unwrap_or_default();
                self.archive_path_input = path.display().to_string();
                let loaded = LoadedArchive { path, manifest };
                self.selected_restore_roots = collect_restore_roots(&loaded);
                self.loaded_archive = Some(loaded);
                self.restore_user_data = true;
                self.restore_portable_apps = true;
                self.skip_existing_restore_files = false;
                self.last_verification = None;
                self.last_restore = None;
                self.last_error = None;
                self.recent_archives = archive::list_recent_archives(8).unwrap_or_default();
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
        self.selected_user_roots.clear();
        self.selected_portable_apps.clear();
        self.last_archive = None;
        self.last_verification = None;
        self.last_restore = None;
        self.last_error = None;
    }

    fn persist_config(&self) -> anyhow::Result<PathBuf> {
        config::save_config(&config::AppConfig {
            selected_user_roots: self.selected_user_roots.clone(),
            selected_portable_apps: self.selected_portable_apps.clone(),
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
}

impl eframe::App for WinRehomeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("WinRehome");
            ui.label("面向 Windows 的迁移备份工具，只保留真正值得迁移的数据。");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("生成预览").clicked() {
                    match plan::build_preview() {
                        Ok(preview) => self.load_preview(preview),
                        Err(error) => {
                            self.last_archive = None;
                            self.last_error = Some(error.to_string());
                        }
                    }
                }

                if ui.button("清空").clicked() {
                    self.clear_preview();
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.label(RichText::new("可靠性原则").strong());
                ui.label("- 已安装软件只记录，不打包程序本体。");
                ui.label("- 便携软件只是候选项，确认后才会进入归档。");
                ui.label("- 个人文件采用白名单思路，不靠“排除 Program Files”这种粗规则。");
                ui.label("- 缓存、临时文件、日志和构建产物默认排除。");
            });

            if let Some(error) = &self.last_error {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(200, 40, 40),
                    format!("Operation failed: {error}"),
                );
            }

            if let Some(result) = &self.last_archive {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(35, 120, 70),
                    format!(
                        "归档已创建：{}（{} 个文件，原始大小 {}，归档大小 {}）",
                        result.archive_path.display(),
                        result.file_count,
                        format_bytes(result.original_bytes),
                        format_bytes(result.stored_bytes)
                    ),
                );
            }

            if let Some(result) = &self.last_restore {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(45, 95, 175),
                    format!(
                        "归档已恢复：{} -> {}（{} 个文件，{}，跳过 {} 个已存在文件）",
                        result.archive_path.display(),
                        result.destination_root.display(),
                        result.restored_files,
                        format_bytes(result.restored_bytes),
                        result.skipped_existing_files
                    ),
                );
            }

            if let Some(result) = &self.last_verification {
                ui.add_space(10.0);
                ui.colored_label(
                    Color32::from_rgb(110, 85, 190),
                    format!(
                        "归档校验完成：{}（{} 个文件，{}）",
                        result.archive_path.display(),
                        result.verified_files,
                        format_bytes(result.verified_bytes)
                    ),
                );
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.group(|ui| {
                ui.label(RichText::new("归档恢复").strong());
                ui.horizontal(|ui| {
                    if ui.button("加载最新归档").clicked() {
                        match archive::find_latest_archive() {
                            Ok(Some(path)) => self.load_archive_from_path(path),
                            Ok(None) => {
                                self.loaded_archive = None;
                                self.last_restore = None;
                                self.last_error =
                                    Some("默认备份目录中没有找到 .wrh 归档。".to_string());
                            }
                            Err(error) => {
                                self.loaded_archive = None;
                                self.last_restore = None;
                                self.last_error = Some(error.to_string());
                            }
                        }
                    }

                    ui.label("归档路径");
                    ui.text_edit_singleline(&mut self.archive_path_input);
                    if ui.button("加载归档").clicked() {
                        let path = PathBuf::from(self.archive_path_input.trim());
                        self.load_archive_from_path(path);
                    }
                });

                if !self.recent_archives.is_empty() {
                    ui.collapsing("Recent archives", |ui| {
                        for path in self.recent_archives.clone() {
                            if ui.button(path.display().to_string()).clicked() {
                                self.load_archive_from_path(path);
                            }
                        }
                    });
                }

                if let Some(loaded) = self.loaded_archive.clone() {
                    ui.add_space(8.0);
                    ui.label(format!("当前归档：{}", loaded.path.display()));
                    ui.label(format!(
                        "创建时间：{} | 文件数：{} | 原始大小：{} | 归档大小：{}",
                        loaded.manifest.created_at_unix,
                        loaded.manifest.files.len(),
                        format_bytes(loaded.manifest.original_bytes),
                        format_bytes(loaded.manifest.stored_bytes)
                    ));
                    ui.label(format!(
                        "已安装软件记录：{} | 用户目录：{} | 便携软件：{}",
                        loaded.manifest.installed_apps.len(),
                        loaded.manifest.selected_user_roots.len(),
                        loaded.manifest.selected_portable_apps.len()
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut self.restore_user_data, "恢复个人文件")
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                        if ui
                            .checkbox(&mut self.restore_portable_apps, "恢复便携软件")
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                        if ui
                            .checkbox(&mut self.skip_existing_restore_files, "跳过已存在文件")
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                    });
                    ui.collapsing("归档内容", |ui| {
                        let all_restore_roots = collect_restore_roots(&loaded);
                        ui.horizontal(|ui| {
                            ui.small(format!(
                                "已选根目录：{} / {}",
                                self.selected_restore_roots.len(),
                                all_restore_roots.len()
                            ));
                            if ui.button("全部选择").clicked() {
                                self.selected_restore_roots = all_restore_roots.clone();
                                let _ = self.persist_config();
                            }
                            if ui.button("全部清空").clicked() {
                                self.selected_restore_roots.clear();
                                let _ = self.persist_config();
                            }
                        });
                        ui.add_space(4.0);
                        if !loaded.manifest.selected_user_roots.is_empty() {
                            ui.label(RichText::new("个人文件目录").strong());
                            for root in &loaded.manifest.selected_user_roots {
                                let key = user_restore_root_key(root);
                                let mut selected = self.selected_restore_roots.contains(&key);
                                if ui.checkbox(&mut selected, &root.label).changed() {
                                    if selected {
                                        self.selected_restore_roots.insert(key.clone());
                                    } else {
                                        self.selected_restore_roots.remove(&key);
                                    }
                                    let _ = self.persist_config();
                                }
                                ui.small(format!("路径：{}", root.path));
                            }
                            ui.add_space(6.0);
                        }
                        if !loaded.manifest.selected_portable_apps.is_empty() {
                            ui.label(RichText::new("便携软件").strong());
                            for app in &loaded.manifest.selected_portable_apps {
                                let key = portable_restore_root_key(app);
                                let mut selected = self.selected_restore_roots.contains(&key);
                                if ui.checkbox(&mut selected, &app.display_name).changed() {
                                    if selected {
                                        self.selected_restore_roots.insert(key.clone());
                                    } else {
                                        self.selected_restore_roots.remove(&key);
                                    }
                                    let _ = self.persist_config();
                                }
                                ui.small(format!("路径：{}", app.root_path));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("校验归档").clicked() {
                            match archive::verify_archive(&loaded.path) {
                                Ok(result) => {
                                    self.last_verification = Some(result);
                                    self.last_error = None;
                                }
                                Err(error) => {
                                    self.last_verification = None;
                                    self.last_error = Some(error.to_string());
                                }
                            }
                        }
                        ui.label("恢复到");
                        if ui
                            .text_edit_singleline(&mut self.restore_destination_input)
                            .changed()
                        {
                            let _ = self.persist_config();
                        }
                        if ui.button("使用默认恢复目录").clicked() {
                            if let Ok(path) = archive::default_restore_dir(&loaded.path) {
                                self.restore_destination_input = path.display().to_string();
                                let _ = self.persist_config();
                            }
                        }
                        let effective_restore_roots = effective_restore_roots(
                            &loaded,
                            self.restore_user_data,
                            self.restore_portable_apps,
                            &self.selected_restore_roots,
                        );
                        let can_restore = !self.restore_destination_input.trim().is_empty()
                            && !effective_restore_roots.is_empty();
                        if ui
                            .add_enabled(can_restore, egui::Button::new("开始恢复"))
                            .clicked()
                        {
                            let destination = PathBuf::from(self.restore_destination_input.trim());
                            let use_default_restore = self.restore_user_data
                                && self.restore_portable_apps
                                && !self.skip_existing_restore_files
                                && self.selected_restore_roots == collect_restore_roots(&loaded);
                            let restore_result = if use_default_restore {
                                archive::restore_archive(&loaded.path, &destination)
                            } else {
                                archive::restore_archive_with_selection(
                                    &loaded.path,
                                    &destination,
                                    archive::RestoreSelection {
                                        restore_user_data: self.restore_user_data,
                                        restore_portable_apps: self.restore_portable_apps,
                                        selected_roots: effective_restore_roots.clone(),
                                        skip_existing_files: self.skip_existing_restore_files,
                                    },
                                )
                            };
                            match restore_result {
                                Ok(result) => {
                                    self.last_restore = Some(result);
                                    self.last_verification = None;
                                    self.last_error = None;
                                    let _ = self.persist_config();
                                }
                                Err(error) => {
                                    self.last_restore = None;
                                    self.last_error = Some(error.to_string());
                                }
                            }
                        }
                    });
                    let effective_restore_count = effective_restore_roots(
                        &loaded,
                        self.restore_user_data,
                        self.restore_portable_apps,
                        &self.selected_restore_roots,
                    )
                    .len();
                    if self.restore_destination_input.trim().is_empty() {
                        ui.small("请先选择恢复目标目录。");
                    } else if effective_restore_count == 0 {
                        ui.small("请至少选择一个已启用的恢复根目录。");
                    } else {
                        ui.small(format!(
                            "已准备恢复 {} 个选中的根目录。",
                            effective_restore_count
                        ));
                    }
                }
            });

            if let Some(preview) = self.preview.clone() {
                let summary = preview
                    .summarize_selection(&self.selected_user_roots, &self.selected_portable_apps);

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                ui.heading("扫描预览");
                ui.label(format!("已安装软件记录：{}", preview.installed_apps.len()));
                ui.label(format!(
                    "便携软件候选：{}",
                    preview.portable_candidates.len()
                ));
                ui.label(format!("高价值用户目录：{}", preview.user_data_roots.len()));
                ui.label(format!("全局排除规则：{}", preview.exclusion_rules.len()));

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("使用推荐选择").clicked() {
                        self.selected_user_roots = preview.default_user_root_keys();
                        self.selected_portable_apps = preview.default_portable_keys();
                        let _ = self.persist_config();
                    }

                    if ui.button("清空选择").clicked() {
                        self.selected_user_roots.clear();
                        self.selected_portable_apps.clear();
                        let _ = self.persist_config();
                    }

                    if ui.button("创建备份归档").clicked() {
                        match archive::create_backup_archive(
                            &preview,
                            &self.selected_user_roots,
                            &self.selected_portable_apps,
                        ) {
                            Ok(result) => {
                                self.load_archive_from_path(result.archive_path.clone());
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

                ui.add_space(10.0);
                ui.group(|ui| {
                    ui.label(RichText::new("当前备份计划").strong());
                    ui.label(format!("保留的用户目录：{}", summary.selected_user_roots));
                    ui.label(format!(
                        "保留的便携软件：{}",
                        summary.selected_portable_apps
                    ));
                    ui.label(format!("预计文件数：{}", summary.total_files));
                    ui.label(format!("预计大小：{}", format_bytes(summary.total_bytes)));
                });

                ui.add_space(10.0);
                ui.columns(3, |columns| {
                    columns[0].group(|ui| {
                        ui.label(RichText::new("已安装软件").strong());
                        for app in preview.installed_apps.iter().take(12) {
                            ui.label(format!("{} [来源: {}]", app.display_name, app.source));
                            if let Some(path) = &app.install_location {
                                ui.small(format!("安装位置：{}", path.display()));
                            }
                            ui.small(format!("注册表键：{}", app.uninstall_key));
                            ui.add_space(4.0);
                        }
                    });

                    columns[1].group(|ui| {
                        ui.label(RichText::new("便携软件候选").strong());
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| {
                                for item in preview.portable_candidates.iter().take(20) {
                                    let key = plan::path_key(&item.root_path);
                                    let mut selected = self.selected_portable_apps.contains(&key);
                                    if ui.checkbox(&mut selected, &item.display_name).changed() {
                                        if self.selected_portable_apps.contains(&key) {
                                            self.selected_portable_apps.remove(&key);
                                        } else {
                                            self.selected_portable_apps.insert(key.clone());
                                        }
                                        let _ = self.persist_config();
                                    }
                                    ui.small(format!("置信度：{}", item.confidence_label()));
                                    ui.small(format!("根目录：{}", item.root_path.display()));
                                    ui.small(format!("主程序：{}", item.main_executable.display()));
                                    ui.small(format!(
                                        "预计大小：{}，共 {} 个文件",
                                        format_bytes(item.stats.total_bytes),
                                        item.stats.file_count
                                    ));
                                    ui.small(format!(
                                        "默认状态：{}",
                                        if item.default_selected {
                                            "已选择"
                                        } else {
                                            "未选择"
                                        }
                                    ));
                                    for reason in item.reasons.iter().take(3) {
                                        ui.small(reason);
                                    }
                                    ui.add_space(6.0);
                                }
                            });
                    });

                    columns[2].group(|ui| {
                        ui.label(RichText::new("用户数据目录").strong());
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for root in &preview.user_data_roots {
                                    let key = plan::path_key(&root.path);
                                    let mut selected = self.selected_user_roots.contains(&key);
                                    if ui.checkbox(&mut selected, root.label).changed() {
                                        if selected {
                                            self.selected_user_roots.insert(key.clone());
                                        } else {
                                            self.selected_user_roots.remove(&key);
                                        }
                                        let _ = self.persist_config();
                                    }
                                    ui.small(format!("分类：{}", root.category));
                                    ui.small(format!("路径：{}", root.path.display()));
                                    ui.small(root.reason);
                                    ui.small(format!(
                                        "预计大小：{}，共 {} 个文件",
                                        format_bytes(root.stats.total_bytes),
                                        root.stats.file_count
                                    ));
                                    ui.small(format!(
                                        "默认状态：{}",
                                        if root.default_selected {
                                            "已选择"
                                        } else {
                                            "未选择"
                                        }
                                    ));
                                    ui.add_space(6.0);
                                }
                            });

                        ui.add_space(10.0);
                        ui.label(RichText::new("排除规则").strong());
                        for rule in &preview.exclusion_rules {
                            ui.small(format!("{}: {}", rule.label, rule.pattern));
                        }
                    });
                });
            }
        });
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
